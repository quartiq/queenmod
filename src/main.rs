#![no_std]
#![no_main]
#![feature(asm)]

#[cfg_attr(feature = "itm", macro_use(iprint))]
extern crate cortex_m;
#[macro_use(entry,exception)]
extern crate cortex_m_rt;
#[cfg(feature = "itm")]
extern crate panic_itm;
#[cfg(not(feature = "itm"))]
extern crate panic_abort;
#[macro_use(interrupt)]
extern crate stm32f4;

use stm32f4::stm32f446;

const N_SAMPLES: usize = 16;

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
         .gpiocen().enabled()
         .dma2en().enabled()
    );
    rcc.apb1enr.modify(|_, w|
        w.tim2en().enabled()
         .dacen().enabled()
    );
    rcc.apb2enr.modify(|_, w|
        w.adc1en().enabled()
    );
}

/// Set up the systick to provide a 1ms timebase
fn systick_init(syst: &mut stm32f446::SYST) {
    syst.set_reload((168_000_000 / 8) / 1000);
    syst.clear_current();
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::External);
    syst.enable_interrupt();
    syst.enable_counter();
}

/// Set up LED for TIM2 OC1
fn io_init(gpioa: &mut stm32f446::GPIOA, gpioc: &mut stm32f446::GPIOC) {
    // PA15: TIM2
    gpioa.moder.modify(|_, w| w.moder15().alternate());
    gpioa.otyper.modify(|_, w| w.ot15().push_pull());
    gpioa.ospeedr.modify(|_, w| w.ospeedr15().low_speed());
    gpioa.afrh.modify(|_, w| w.afrh15().af1());

    // PA0: ADC_IN0
    gpioa.moder.modify(|_, w| w.moder0().analog());
    gpioa.pupdr.modify(|_, w| w.pupdr0().floating());

    // PA4: DAC_OUT1
    gpioa.moder.modify(|_, w| w.moder4().analog());
    gpioa.pupdr.modify(|_, w| w.pupdr4().floating());

    // PA5: DAC_OUT2
    gpioa.moder.modify(|_, w| w.moder5().analog());
    gpioa.pupdr.modify(|_, w| w.pupdr5().floating());

    // PA10: ISR duty
    gpioa.moder.modify(|_, w| w.moder10().output());
    gpioa.otyper.modify(|_, w| w.ot10().push_pull());
    gpioa.ospeedr.modify(|_, w| w.ospeedr10().low_speed());

    // PC13: MODE (user button), external pullup
    gpioc.moder.modify(|_, w| w.moder13().input());
    gpioc.pupdr.modify(|_, w| w.pupdr13().floating());
}

/// Set up timer TIM2 to emit square pulses on OC1
fn tim2_init(tim2: &mut stm32f446::TIM2) {
    tim2.psc.write(|w| unsafe { w.psc().bits(4 * (12 + 3) - 1) });
    tim2.arr.write(|w| unsafe { w.arr_l().bits(N_SAMPLES as u16 - 1) } );
    tim2.ccr1.write(|w| unsafe { w.ccr1_l().bits(N_SAMPLES as u16 / 2) } );
    tim2.ccmr1_output.modify(|_, w| unsafe {
        w.oc1m().bits(0b110)
         .oc1pe().set_bit() });
    tim2.ccer.modify(|_, w|
        w.cc1p().clear_bit()  // active high
         .cc1e().set_bit());  // enable
    tim2.egr.write(|w| w.ug().set_bit());
    tim2.cr2.modify(|_, w| unsafe {
        w.mms().bits(0b010) });  // UEV
    tim2.cr1.modify(|_, w| unsafe {
        w.ckd().bits(0)  // div1
         .dir().clear_bit()  // up
         .arpe().set_bit()  // auto preload
         .cen().set_bit() });  // enable
}

/// Set up ADC1 to sample from Pxx
fn adc1_init(adc_common: &mut stm32f446::ADC_COMMON, adc1: &mut stm32f446::ADC1) {
    adc_common.ccr.modify(|_, w|
        w.adcpre().div4()
         .tsvrefe().enabled());
    adc1.cr2.modify(|_, w|
        w.cont().single()
         .exten().rising_edge()
         .dma().enabled()
         .eocs().each_sequence()
         .align().right()
         .extsel().tim2trgo()
         .dds().continuous()
         .adon().enabled());
    adc1.cr1.modify(|_, w|
        w.res().twelve_bit()
         .scan().enabled()
         .discen().disabled()
         .discnum().bits(0));
    adc1.smpr2.modify(|_, w|
        w.smp0().cycles3());
    adc1.smpr1.modify(|_, w|
        w.smp18().cycles480());
    adc1.sqr3.modify(|_, w| unsafe {
        w.sq1().bits(0)
         .sq2().bits(0)
         .sq3().bits(0)
         .sq4().bits(0)
         .sq5().bits(0)
         .sq6().bits(0) });
    adc1.sqr2.modify(|_, w| unsafe {
        w.sq7().bits(0)
         .sq8().bits(0)
         .sq9().bits(0)
         .sq10().bits(0)
         .sq11().bits(0)
         .sq12().bits(0) });
    adc1.sqr1.modify(|_, w| unsafe {
        w.sq13().bits(0)
         .sq14().bits(0)
         .sq15().bits(0)
         .sq16().bits(0)
         .l().bits(N_SAMPLES as u8 - 1) });
}

/// Set up LED for TIM2 OC1
fn dac_init(dac: &mut stm32f446::DAC) {
    dac.cr.modify(|_, w|
        w.tsel1().tim2_trgo()
         .wave1().disabled()
         .mamp1().bits(0)
         .ten1().enabled()
         .en1().enabled()
         .boff1().enabled()
         .tsel2().tim2_trgo()
         .wave2().disabled()
         .mamp2().bits(0)
         .ten2().enabled()
         .en2().enabled()
         .boff2().enabled());
}

static mut ADC_SAMPLES: [FIRState; 2] = [[0; N_SAMPLES]; 2];

/// Set up both DAC channels
fn dma2_init(dma2: &mut stm32f446::DMA2, par: u32) {
    dma2.s4cr.modify(|_, w|
        w.chsel().bits(0)  // ADC1
         .dbm().enabled()
         .pl().very_high()
         .msize().half_word()
         .psize().half_word()
         .minc().incremented()
         .pinc().fixed()
         .circ().enabled()
         .mburst().single()
         .pburst().single()
         .dir().peripheral_to_memory()
         .pfctrl().dma()
         .tcie().enabled()
         .teie().enabled()
         .dmeie().enabled()
    );
    dma2.s4par.write(|w| w.pa().bits(par));
    let mar0 = unsafe { &ADC_SAMPLES[0] } as *const _ as u32;
    dma2.s4m0ar.write(|w| w.m0a().bits(mar0));
    let mar1 = unsafe { &ADC_SAMPLES[1] } as *const _ as u32;
    dma2.s4m1ar.write(|w| w.m1a().bits(mar1));
    dma2.s4ndtr.write(|w| w.ndt().bits(N_SAMPLES as u16));
    dma2.s4fcr.modify(|_, w| w.dmdis().enabled());
    dma2.s4cr.modify(|_, w| w.en().enabled());
}

entry!(main);
fn main() -> ! {
    cortex_m::interrupt::free(|_cs| {
        let mut peripherals = stm32f446::Peripherals::take().unwrap();
        let mut core_peripherals = cortex_m::Peripherals::take().unwrap();

        rcc_init(&mut peripherals);
        systick_init(&mut core_peripherals.SYST);
        println!("Version {} {}", build_info::PKG_VERSION, build_info::GIT_VERSION.unwrap());
        println!("Platform {}", build_info::TARGET);
        println!("Built on {}", build_info::BUILT_TIME_UTC);
        println!("{}", build_info::RUSTC_VERSION);
        println!("Ready.\n");

        io_init(&mut peripherals.GPIOA, &mut peripherals.GPIOC);
        dac_init(&mut peripherals.DAC);
        dma2_init(&mut peripherals.DMA2, &peripherals.ADC1.dr as *const _ as u32);
        core_peripherals.NVIC.clear_pending(stm32f446::Interrupt::DMA2_STREAM4);
        core_peripherals.NVIC.enable(stm32f446::Interrupt::DMA2_STREAM4);
        adc1_init(&mut peripherals.ADC_COMMON, &mut peripherals.ADC1);
        tim2_init(&mut peripherals.TIM2);

        core_peripherals.DWT.enable_cycle_counter();
    });
    loop {
        cortex_m::asm::wfi();
    }
}

/*
#[link_name = "llvm.arm.smlald"]
pub fn arm_smlald(a: i16x2, b: i16x2, c: i64) -> i64;
*/

fn macc(y0: i16, x: &[i16], a: &[i16], shift: u8) -> i16 {
    let y = match () {
        #[cfg(not(feature = "simd"))]
        _ => {
            ((y0 as i32) << shift) + x.iter()
              .zip(a.iter())
              .map(|(&i, &j)| (i as i32) * (j as i32))
              .sum::<i32>()
        },
        #[cfg(feature = "simd")]
        _ => {
            assert_eq!(x.len(), a.len());
            let mut y = ((y0 as i32) << shift, 0i32);
            unsafe {
                for i in 0..x.len()/2 {
                    let xi = *(x.get_unchecked(i*2) as *const _ as *const i32);
                    let ai = *(a.get_unchecked(i*2) as *const _ as *const i32);
                    asm!("smlald $0, $1, $2, $3"
                        : "=r"(y.0), "=r"(y.1)
                        : "r"(xi), "r"(ai), "0"(y.0), "1"(y.1));
                }
            }
            if x.len() & 1 == 1 {
                y.0 += (x[x.len() - 1] as i32)*(a[a.len() - 1] as i32);
            }
            y.0
        }
    };
    (y >> shift).max(i16::min_value() as i32).min(i16::max_value() as i32) as i16
}

type IIRState = [i16; 5];

struct IIR {
    ba: IIRState,
    shift: u8,
}

impl IIR {
    fn update(&self, xy: &mut IIRState, x0: i16) -> i16 {
        xy.rotate_right(1);
        xy[0] = x0;
        let y0 = macc(0, xy, &self.ba, self.shift);
        xy[xy.len()/2] = y0;
        y0
    }
}

type FIRState = [i16; N_SAMPLES];

struct FIR {
    a: FIRState,
    offset: i16,
    shift: u8,
}

impl FIR {
    #[inline(never)]
    fn apply(&self, x: &FIRState) -> i16 {
        macc(self.offset, x, &self.a, self.shift)
    }
}

static mut FIR_MODE: usize = 0;
const FIR_LEN: usize = 5;
static FIRX: [[FIR; 2]; FIR_LEN] = [
    [ // fundamental t_mod
        FIR{ shift: 11, offset: 0, a:
                [0, 1247, 2304, 3011, 3259, 3011, 2304, 1247,
                    0, -1247, -2304, -3011, -3259, -3011, -2304, -1247] },
        FIR{ shift: 11, offset: 0, a:
                [-3259, -3011, -2304, -1247, 0, 1247, 2304, 3011,
                    3259, 3011, 2304, 1247, 0, -1247, -2304, -3011] },
    ],
    [ // second harmonic t_mod/2
        FIR{ shift: 11, offset: 0, a:
                [0, 2399, 3393, 2399, 0, -2399, -3393, -2399,
                    0, 2399, 3393, 2399, 0, -2399, -3393, -2399] },
        FIR{ shift: 11, offset: 0, a:
                [-3393, -2399, 0, 2399, 3393, 2399, 0, -2399,
                    -3393, -2399, 0, 2399, 3393, 2399, 0, -2399] },
    ],
    [ // third harmonic t_mod/3
        FIR{ shift: 11, offset: 0, a:
                [0, 3011, 2304, -1247, -3259, -1247, 2304, 3011,
                    0, -3011, -2304, 1247, 3259, 1247, -2304, -3011] },
        FIR{ shift: 11, offset: 0, a:
                [-3259, -1247, 2304, 3011, 0, -3011, -2304, 1247,
                    3259, 1247, -2304, -3011, 0, 3011, 2304, -1247] },
    ],
    [ // square fundamental
        FIR{ shift: 0, offset: 0, a:
                [1, 1, 1, 1, 1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1, -1] },
        FIR{ shift: 0, offset: 0, a:
                [1, 1, 1, 1, -1, -1, -1, -1, -1, -1, -1, -1, 1, 1, 1, 1] },
    ],
    [ // zero and dc
        FIR{ shift: 0, offset: 0, a:
                [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0] },
        FIR{ shift: 4, offset: 0, a:
                [1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1] },
    ],
];

static mut IIR_MODE: usize = 0;
const IIR_LEN: usize = 3;
static IIRX: [[IIR; 2]; IIR_LEN] = [
    [// butter(4, .2)
        IIR{ shift: 12, ba: [200, 400, 200, 4295, -1213] },
        IIR{ shift: 12, ba: [4096, 8192, 4096, 5410, -2592] },
    ],
    [ // id
        IIR{ shift: 0, ba: [1, 0, 0, 0, 0] },
        IIR{ shift: 0, ba: [1, 0, 0, 0, 0] },
    ],
    [ // PII
        IIR{ shift: 13, ba: [400, 400, 0, 8184, 0] },
        IIR{ shift: 0, ba: [1, 0, 0, 0, 0] },
    ],
];

interrupt!(DMA2_STREAM4, dma2_stream4,
           state: [[IIRState; 2]; 2] = [[[0; 5]; 2]; 2]);
fn dma2_stream4(iir_state: &mut [[IIRState; 2]; 2]) {
    #[cfg(feature = "bkpt")]
    cortex_m::asm::bkpt();
    let mut peripherals = unsafe { stm32f446::Peripherals::steal() };
    // let gpioa = &mut peripherals.GPIOA;
    // gpioa.bsrr.write(|w| w.bs10().set());

    let dma = &mut peripherals.DMA2;
    let hisr = dma.hisr.read();

    /*
    let adc = &mut peripherals.ADC1;
    if adc.sr.read().ovr().bit_is_set() {
        println!("x");
        // adc.cr2.modify(|_, w| w.dma().clear_bit());
        // dma.s4cr.modify(|_, w| w.en().disabled());
        let mar = unsafe { &ADC_SAMPLES as *const _ } as u32;
        dma.s4m0ar.write(|w| w.m0a().bits(mar));
        dma.s4m1ar.write(|w| w.m1a().bits(mar + N_SAMPLES as u32*2));
        dma.s4ndtr.write(|w| w.ndt().bits(N_SAMPLES as u16));
        adc.sr.modify(|_, w| w.ovr().clear_bit());
        //dma.s4cr.modify(|_, w| w.en().enabled());
        //adc.cr2.modify(|_, w| w.dma().set_bit());
        //return;
    }

    if hisr.teif4().bit_is_set() {
        println!("t");
    }
    if hisr.dmeif4().bit_is_set() {
        println!("d");
    }
    */

    if hisr.tcif4().bit_is_set() {
        dma.hifcr.write(|w| w.ctcif4().set_bit());
        let ct = 1 - dma.s4cr.read().ct() as usize;
        let mut y: [i16; 2] = [0; 2];
        let a = unsafe { &ADC_SAMPLES[ct] };
        let fir = &FIRX[unsafe { FIR_MODE }];
        let iir = &IIRX[unsafe { IIR_MODE }];
        for i in 0..2 {
            y[i] = fir[i].apply(a);
            for j in 0..2 {
                y[i] = iir[j].update(&mut iir_state[i][j], y[i]);
            }
            y[i] = (y[i] >> 4) + 0x800;
        }
        peripherals.DAC.dhr12rd.write(|w| unsafe {
            w.dacc1dhr().bits(y[0] as u16)
             .dacc2dhr().bits(y[1] as u16) });

        // let itm = &mut core_peripherals.ITM;
        // while ! itm.stim[0].is_fifo_ready() {}
        // itm.stim[0].write_u32(((y[1] as u32) << 16) | ((y[0] as u32) & 0xffff));
    }
    // gpioa.bsrr.write(|w| w.br10().reset());
    #[cfg(feature = "bkpt")]
    cortex_m::asm::bkpt();
}

exception!(SysTick, sys_tick, state: u32 = 0);
fn sys_tick(t: &mut u32) {
    let peripherals = unsafe { stm32f446::Peripherals::steal() };
    match debounce(peripherals.GPIOC.idr.read().idr13().bit_is_clear(), t) {
        Some(Debounce::Short) =>
            unsafe { FIR_MODE = (FIR_MODE + 1) % FIRX.len(); },
        Some(Debounce::Long) =>
            unsafe { IIR_MODE = (IIR_MODE + 1) % IIRX.len(); },
        None => ()
    };
}


enum Debounce {
    Short,
    Long,
}

fn debounce(signal: bool, time: &mut u32) -> Option<Debounce> {
    let t_long = 300;
    let t_short = 40;
    let (t, ret) = match (signal, *time) {
        (true, t) if t < t_long => (t + 1, None),
        (true, t) if t == t_long => (t + 1, Some(Debounce::Long)),
        (false, t) if t > t_long => (t_short/2, None),
        (false, t) if t >= t_short => (t_short/2, Some(Debounce::Short)),
        (false, t) if t > 0 => (t - 1, None),
        (_, t) => (t, None),
    };
    *time = t;
    ret
}

exception!(HardFault, hard_fault);
fn hard_fault(ef: &cortex_m_rt::ExceptionFrame) -> ! {
    panic!("HardFault at {:#?}", ef);
}

exception!(*, default_handler);
fn default_handler(irqn: i16) {
    panic!("Unhandled exception (IRQn = {})", irqn);
}
