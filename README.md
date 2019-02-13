# PWM+DDSM3 hybrid

* nucleo64 stm32f446
* rust

# Design

# Build

## Features

* **itm**: use the ITM cell for debugging output
* **bkpt**: place breakpoints around the ISR for timing

## Commands

```
rustup override add nightly
rustup install nightly
rustup target add thumbv7em-none-eabi

cargo install itm  # features=itm
mkfifo itm.fifo  # features=itm
openocd -f stm32f446-nucleo64.cfg
cargo run --release
itmdump -f itm.fifo
```

# TODO
