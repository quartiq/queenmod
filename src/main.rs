#![no_std]
#![no_main]
#![feature(asm)]

extern crate cortex_m;
extern crate cortex_m_rt;
#[cfg(feature = "itm")]
extern crate panic_itm;
#[cfg(not(feature = "itm"))]
extern crate panic_abort;
extern crate stm32f4;

#[cfg(feature = "itm")]
use cortex_m::iprint;
use cortex_m_rt::{entry, exception};
use stm32f4::{stm32f446, stm32f446::interrupt};

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        #[cfg(feature = "itm")]
        {
            use cortex_m;
            let stim = unsafe { &mut (*cortex_m::peripheral::ITM::ptr()).stim[0] };
            iprint!(stim, $($arg)*);
        }
    })
}

#[macro_export]
macro_rules! println {
    ($fmt:expr) => (print!(concat!($fmt, "\n")));
    ($fmt:expr, $($arg:tt)*) => (print!(concat!($fmt, "\n"), $($arg)*));
}

// Pull in build information (from `built` crate)
mod build_info {
    #![allow(dead_code)]
    include!(concat!(env!("OUT_DIR"), "/built.rs"));
}

/// Set up PLL to 168MHz from 16MHz HSI
fn rcc_init(peripherals: &mut stm32f446::Peripherals) {
    let rcc = &peripherals.RCC;
    let flash = &peripherals.FLASH;

    // Reset all peripherals
    rcc.ahb1rstr.write(|w| unsafe { w.bits(0xFFFF_FFFF) });
    rcc.ahb1rstr.write(|w| unsafe { w.bits(0)});
    rcc.ahb2rstr.write(|w| unsafe { w.bits(0xFFFF_FFFF) });
    rcc.ahb2rstr.write(|w| unsafe { w.bits(0)});
    rcc.ahb3rstr.write(|w| unsafe { w.bits(0xFFFF_FFFF) });
    rcc.ahb3rstr.write(|w| unsafe { w.bits(0)});
    rcc.apb1rstr.write(|w| unsafe { w.bits(0xFFFF_FFFF) });
    rcc.apb1rstr.write(|w| unsafe { w.bits(0)});
    rcc.apb2rstr.write(|w| unsafe { w.bits(0xFFFF_FFFF) });
    rcc.apb2rstr.write(|w| unsafe { w.bits(0)});

    // Ensure HSI is on and stable
    rcc.cr.modify(|_, w| w.hsion().set_bit());
    while rcc.cr.read().hsion().bit_is_clear() {}

    // Set system clock to HSI
    rcc.cfgr.modify(|_, w| w.sw().hsi());
    while !rcc.cfgr.read().sws().is_hsi() {}

    // Clear registers to reset value
    rcc.cr.write(|w| w.hsion().set_bit());
    rcc.cfgr.reset();

    // Configure PLL: 16MHz /8 *168 /2, source HSI
    rcc.pllcfgr.write(|w| unsafe {
        w.pllq().bits(4)
         .pllsrc().hsi()
         .pllp().div2()
         .plln().bits(168)
         .pllm().bits(8)
    });
    // Activate PLL
    rcc.cr.modify(|_, w| w.pllon().set_bit());

    // Set other clock domains: PPRE2 to /2, PPRE1 to /4, HPRE to /1
    rcc.cfgr.modify(|_, w|
        w.ppre2().div2()
         .ppre1().div4()
         .hpre().div1());

    // Flash setup: I$ and D$ enabled, prefetch enabled, 5 wait states (OK for 3.3V at 168MHz)
    flash.acr.write(|w| unsafe {
        w.icen().set_bit()
         .dcen().set_bit()
         .prften().set_bit()
         .latency().bits(5)
    });

    // Swap system clock to PLL
    rcc.cfgr.modify(|_, w| w.sw().pll());
    while !rcc.cfgr.read().sws().is_pll() {}

    // Set up peripheral clocks
    rcc.ahb1enr.modify(|_, w|
        w.gpioaen().enabled()
         .dma1en().enabled()
    );
    rcc.apb1enr.modify(|_, w|
        w.tim3en().enabled()
    );
}

/// Set IOs
fn io_init(gpioa: &mut stm32f446::GPIOA) {
    // PA6: TIM3_CH1
    gpioa.moder.modify(|_, w| w.moder6().alternate());
    gpioa.otyper.modify(|_, w| w.ot6().push_pull());
    gpioa.ospeedr.modify(|_, w| w.ospeedr6().low_speed());
    gpioa.afrl.modify(|_, w| w.afrl6().af2());
}

const M_PWM: u32 = 8;

/// Set up timer TIM3 to PWM on OC1
fn tim3_init(tim3: &mut stm32f446::TIM3) {
    tim3.psc.write(|w| unsafe { w.psc().bits(0) });
    tim3.arr.write(|w| w.arr().bits((1 << M_PWM) - 1));
    tim3.ccr1.write(|w| w.ccr1().bits(0));
    tim3.ccmr1_output.modify(|_, w| unsafe {
        w.oc1m().bits(0b110)  // PWM1
         .oc1pe().set_bit() }); // CCR1 preload
    tim3.ccer.modify(|_, w|
        w.cc1p().clear_bit()  // active high
         .cc1e().set_bit());  // enable
    tim3.cr2.modify(|_, w|
        w.mms().reset()   // Master mode reset TRGO=EGR
         .ccds().on_update());  // DMA on update
    tim3.dier.modify(|_, w|
        w.cc1de().set_bit());  // CC1 event DMA
    tim3.dcr.modify(|_, w| unsafe {
        w.dba().bits(0x34 >> 2)  // start at ccr1
         .dbl().bits(0) });  // one xfer
    tim3.egr.write(|w| w.ug().update());
    tim3.cr1.modify(|_, w|
        w.ckd().not_divided()
         .dir().up()
         .arpe().enabled()  // auto preload
         .cen().enabled());  // enable
}

const N_PWM: usize = 1 << 8;
static mut PWM_SAMPLES: [[u16; N_PWM]; 2] = [[0; N_PWM]; 2];

/// Set up PWM DMA
fn dma1_init(dma1: &mut stm32f446::DMA1, par: u32) {
    dma1.s4cr.modify(|_, w|
        w.chsel().bits(5)  // TIM3_CH1/TRG
         .dbm().enabled()
         .pl().very_high()
         .msize().half_word()
         .psize().half_word()
         .minc().incremented()
         .pinc().fixed()
         .circ().enabled()
         .mburst().single()
         .pburst().single()
         .dir().memory_to_peripheral()
         .pfctrl().dma()
         .tcie().enabled()  // transfer complete
         .teie().enabled()  // error
         .dmeie().enabled()  // direct mode error
    );
    dma1.s4par.write(|w| w.pa().bits(par));
    let mar0 = unsafe { &PWM_SAMPLES[0] } as *const _ as u32;
    dma1.s4m0ar.write(|w| w.m0a().bits(mar0));
    let mar1 = unsafe { &PWM_SAMPLES[1] } as *const _ as u32;
    dma1.s4m1ar.write(|w| w.m1a().bits(mar1));
    dma1.s4ndtr.write(|w| w.ndt().bits(N_PWM as u16));
    dma1.s4fcr.modify(|_, w| w.dmdis().enabled());  // direct mode enabled
    dma1.s4cr.modify(|_, w| w.en().enabled());
}

#[entry]
fn main() -> ! {
    cortex_m::interrupt::free(|_cs| {
        let mut peripherals = stm32f446::Peripherals::take().unwrap();
        let mut core_peripherals = cortex_m::Peripherals::take().unwrap();

        rcc_init(&mut peripherals);
        println!("Version {} {}", build_info::PKG_VERSION, build_info::GIT_VERSION.unwrap());
        println!("Platform {}", build_info::TARGET);
        println!("Built on {}", build_info::BUILT_TIME_UTC);
        println!("{}", build_info::RUSTC_VERSION);

        core_peripherals.DWT.enable_cycle_counter();
        io_init(&mut peripherals.GPIOA);

        dma1_init(&mut peripherals.DMA1, &peripherals.TIM3.dmar as *const _ as u32);
        stm32f446::NVIC::unpend(stm32f446::Interrupt::DMA1_STREAM4);
        core_peripherals.NVIC.enable(stm32f446::Interrupt::DMA1_STREAM4);
        tim3_init(&mut peripherals.TIM3);
    });
    loop {
        cortex_m::asm::wfi();
    }
}

struct DDSM3 {
    lfsr: u16,
    a: [u16; 3],
    c: [i8; 2],
}

impl DDSM3 {
    fn process(&mut self, x: u16) -> i8 {
        let bit = ((self.lfsr >> 15) ^ (self.lfsr >> 14) ^
                   (self.lfsr >> 12) ^ (self.lfsr >> 3)) & 1;
        self.lfsr = (self.lfsr << 1) ^ bit;
        let (x, c2) = self.a[0].overflowing_add(x ^ bit);
        self.a[0] = x;
        let (x, c1) = self.a[1].overflowing_add(x);
        self.a[1] = x;
        let (x, c0) = self.a[2].overflowing_add(x);
        self.a[2] = x;
        let c1 = c0 as i8 - self.c[0] + c1 as i8;
        self.c[0] = c0 as i8;
        let c2 = c1 - self.c[1] + c2 as i8;
        self.c[1] = c1;
        c2
    }
}

const N_COS: usize = 36;
static COS: [i32; N_COS] = [
    0xfae147, 0xf9035f, 0xf3782c, 0xea6acd, 0xde21ac, 0xcefc59,
    0xbd70a3, 0xaa0705, 0x95567f, 0x800000, 0x6aa980, 0x55f8fa,
    0x428f5c, 0x3103a6, 0x21de53, 0x159532, 0xc87d3, 0x6fca0,
    0x51eb8, 0x6fca0, 0xc87d3, 0x159532, 0x21de53, 0x3103a6,
    0x428f5c, 0x55f8fa, 0x6aa980, 0x7fffff, 0x95567f, 0xaa0705,
    0xbd70a3, 0xcefc59, 0xde21ac, 0xea6acd, 0xf3782c, 0xf9035f];

#[interrupt]
fn DMA1_STREAM4() {
    static mut DSM: DDSM3 = DDSM3{ lfsr: 1, a: [0, 0, 0], c: [0, 0] };
    static mut I: usize = 0;
    #[cfg(feature = "bkpt")]
    cortex_m::asm::bkpt();
    let mut peripherals = unsafe { stm32f446::Peripherals::steal() };

    let dma = &mut peripherals.DMA1;
    let hisr = dma.hisr.read();

    if hisr.tcif4().bit_is_set() {
        let ct = 1 - dma.s4cr.read().ct() as usize;
        let c = COS[*I];
        // let c = (c >> 8) + 0x800000;  // only dsm bit
        for i in 0..N_PWM {
            let y = (c >> 16) + DSM.process(c as u16) as i32;
            unsafe { PWM_SAMPLES[ct][i] = y as u16 };
        }
        *I = (*I + 1) % N_COS;
        dma.hifcr.write(|w| w.ctcif4().set_bit());
    }

    #[cfg(feature = "bkpt")]
    cortex_m::asm::bkpt();
}

#[exception]
fn HardFault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    panic!("HardFault at {:#?}", ef);
}

#[exception]
fn DefaultHandler(irqn: i16) {
    panic!("Unhandled exception (IRQn = {})", irqn);
}
