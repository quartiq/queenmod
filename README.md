# QUEEN WMS modulator/demodulator

* nucleo64 stm32f446
* rust

# Design

## Modulation

* GPIO PA15 square phase modulation ~100 kHz
* has zeros at even harmonics (dick-effect)
* maximum power in the relevant sidebands (especially given fixed amplitude,
  not fixed rms power), zero carrier, zero even harmonics

## Detection

* PA0 ADC input
* sample rate ~1 MHz
* DMA

## Demod, filtering

* frequency shifted rectangular window
* has zeros at multiples of the modulation (especially 2f/3f/dick-like effect)
* highest gain
* lowest noise bw
* scallopping loss not problematic
* sidelobes not problematic
* demodulation IQ or higher orders, or square, or dc/zero/avg

## IIR filtering

* anything goes

## Output

* DAC output PA4, PA5

# Build Notes

```
cargo install itm

rustup override add nightly
rustup install nightly
rustup target add thumbv7m-none-eabi

mkfifo itm.fifo
openocd -f stm32f446-nucleo64.cfg
cargo run --release
itmdump -f itm.fifo
```

# TODO

* ADC1,2 should be interleaved
  * use 15 sample+acquisition cycles, 17+x sample interval
  * use either
    * continuous mode with DDS
    * alternate trigger mode and a 1/n trigger from TIMx, TIMx synced to TIM2
* maybe:
  * interpolate DAC samples
  * DMA double buffer write to DAC with TIMy, TIMy synced to TIM2
