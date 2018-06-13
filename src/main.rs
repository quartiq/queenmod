#![no_std]

extern crate cortex_m;
extern crate cortex_m_semihosting;
extern crate panic_semihosting;

extern crate stm32f4;

extern crate byteorder;

use stm32f4::stm32f427;

/// Try to print over semihosting if a debugger is available
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ({
        use core::fmt::Write;
        use cortex_m;
        use cortex_m_semihosting;
        if unsafe { (*cortex_m::peripheral::DCB::ptr()).dhcsr.read() & 1 == 1 } {
            match cortex_m_semihosting::hio::hstdout() {
                Ok(mut stdout) => {write!(stdout, $($arg)*).ok();},
                Err(_) => ()
            };
        }
    })
}

/// Try to print a line over semihosting if a debugger is available
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
fn rcc_init(peripherals: &mut stm32f427::Peripherals) {
    let rcc = &peripherals.RCC;
    let flash = &peripherals.FLASH;
    let syscfg = &peripherals.SYSCFG;

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
    rcc.cfgr.write(|w| unsafe { w.bits(0) });

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

    // Set SYSCFG early to RMII mode
    rcc.apb2enr.modify(|_, w| w.syscfgen().enabled());
    syscfg.pmc.modify(|_, w| w.mii_rmii_sel().set_bit());

    // Set up peripheral clocks
    rcc.ahb1enr.modify(|_, w|
        w.gpioaen().enabled()
         .gpioben().enabled()
         .gpiocen().enabled()
         .gpioden().enabled()
         .gpioeen().enabled()
         .gpiogen().enabled()
         .crcen().enabled()
    );
}

/// Set up the systick to provide a 1ms timebase
fn systick_init(syst: &mut stm32f427::SYST) {
    syst.set_reload((168_000_000 / 8) / 1000);
    syst.clear_current();
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::External);
    syst.enable_interrupt();
    syst.enable_counter();
}

fn main() {
    let mut peripherals = stm32f427::Peripherals::take().unwrap();
    let mut core_peripherals = stm32f427::CorePeripherals::take().unwrap();

    println!("Hello world.");
    println!("Version {} {}", build_info::PKG_VERSION, build_info::GIT_VERSION.unwrap());
    println!("Platform {}", build_info::TARGET);
    println!("Built on {}", build_info::BUILT_TIME_UTC);
    println!("{}", build_info::RUSTC_VERSION);

    print!(  " Initialising clocks...               ");
    rcc_init(&mut peripherals);
    println!("OK");

    // Turn on STATUS LED
    println!(" Ready.\n");

    // Begin periodic tasks via systick
    systick_init(&mut core_peripherals.SYST);

    // When main returns, cortex-m-rt goes into an infinite wfi() loop.
}
