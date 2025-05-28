# Desk Pal

A friend for your desk top!

## Controlling the Desk Pal

You can control the mouth of the desk pal by sending an 8 bit little endien integer over serial. 0 is fully closed and 255 is fully open.

See `./example.py` for an example.

## Hardware

- The servo is a Dynamixel XL330-M288-T controlled by a Raspberry Pi Pico 2
  - Pin 0: UART TX -> servo RX
  - Pin 5: Audio bit
  - Pin 6: Audio LR clock pin
  - Pin 7: Audio data
- The rest is 3D printed.

### Interfaces

- The Pico uses USB CDC to expose a serial interface

### Flashing the Pico

#### With debug probe

Install [probe-rs](https://probe.rs/), connect to the SWD port of the Pico, and run:

```bash
cd microcontroller
cargo run --release
```

#### Via picotool

Install [picotool](https://github.com/raspberrypi/picotool), then run:

```bash
cd microcontroller
cargo build --release
picotool load target/thumbv8m.main-none-eabihf/release/microcontroller -t elf
```

### T-Rex 3D Model

The T-Rex model in `TRexDeskPal.stl` is based on a scan from the Smithsonian.

- [source](https://3d.si.edu/object/3d/tyrannosaurus-and-triceratops:d8c62d28-4ebc-11ea-b77f-2e728ce88125)
- [source GUID](http://n2t.net/ark:/65665/381d4ee9c-91b4-4ce0-89eb-9053746a9351)
