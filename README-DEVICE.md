# Connections

## Serial
* UART0 (`GP0`/`GP1`) - connected to mux on CH340C on picocalc
* UART1 (`GP8`/`GP9`) - connected to `M_UART3` aka `Serial1` on picocalc mcu. Default mcu firmware writes pmu debug logs to this.

## I2C
* I2C1 (`GP6`/`GP7`) - I2C bus connected to picocalc keyboard/pmu mcu `M_I2C1`

## LCD
* `GP10` - `SPI1_SCK`
* `GP11` - `SPI1_TX`
* `GP12` - `SPI1_RX`
* `GP13` - `SPI1_CS`
* `GP14` - `LCD_DC`
* `GP15` - `LCD_RST`

## Audio
GP26/GP27 - `PWM_L`/`PWM_R` on picocalc audio circuitry

## Microphone (I2S, push-to-talk)
* `GP16` - I2S `BCLK` (bit clock, firmware drives this)
* `GP17` - I2S `WS`/`LRCLK` (word select, firmware drives this)
* `GP18` - I2S `SD` (serial data, mic drives this - the only actual input line)
* `GP19` - spare (unused, or wire as a mic enable/mute line)
* `GP22` - spare (unused)

These are the pins formerly used by the (now removed) SD card reader; see
AGENTS.md for why they were repurposed.

## PSRAM
* `GP2`  - `RAM_TX`
* `GP3`  - `RAM_RX`
* `GP4`  - `RAM_IO2`  - quad mode
* `GP5`  - `RAM_IO3`  - quad mode
* `GP20` - `RAM_CS`
* `GP21` - `RAM_SCK`

Note that all except the CS are exposed to expansion/jumper block.

## Expansion Port/Jumper block
* `GP2`  - Also connected to PSRAM
* `GP3`  - Also connected to PSRAM
* `GP4`  - Also connected to PSRAM
* `GP5`  - Also connected to PSRAM
* `GP21` - Also connected to PSRAM
* `GP28`

