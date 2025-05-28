#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::PIO0;
use embassy_rp::peripherals::UART0;
use embassy_rp::peripherals::USB;
use embassy_rp::pio::{InterruptHandler, Pio};
use embassy_rp::pio_programs::i2s::{PioI2sOut, PioI2sOutProgram};
use embassy_rp::usb::{Driver as UsbDriver, Instance as UsbInstance};
use embassy_usb::class::cdc_acm::{CdcAcmClass, State as SerialState};
use embassy_usb::class::uac1::FeedbackRefresh;
use embassy_usb::class::uac1::speaker::{Speaker, State as SpeakerState};
use embassy_usb::{Builder, Config};
use {defmt_rtt as _, panic_probe as _};

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => embassy_rp::usb::InterruptHandler<USB>;
    UART0_IRQ => embassy_rp::uart::InterruptHandler<UART0>;
    PIO0_IRQ_0 => InterruptHandler<PIO0>;
});

const MAX_CDC_PACKET_SIZE: usize = 64;
const MAX_AUDIO_PACKET_SIZE: usize = 384;

const AUDIO_SAMPLE_RATE: u32 = 48_000;
const AUDIO_BIT_DEPTH: u32 = 16;

/// The ID of the servo to drive.
const SERVO_ID: u32 = 1;

/// The servo position is 0 to 4095
const MAX_POSITION: u32 = 4095;

fn t_rex_angle_mapping(ratio: f32) -> Result<f32, &'static str> {
    if !(0.0..=1.0).contains(&ratio) {
        return Err("Ratio must be between 0.0 and 1.0");
    }
    Ok(ratio * 0.05 + 0.45)
}

fn float_to_servo_range(angle: f32) -> u32 {
    if !(0.0..=1.0).contains(&angle) {
        panic!("Angle must be between 0.0 and 1.0");
    }
    (angle * (MAX_POSITION as f32) + 0.5) as u32
}

async fn send_serial_command(
    tx: &mut embassy_rp::uart::UartTx<'_, embassy_rp::uart::Async>,
    packet: &[u8],
) -> Result<(), embassy_rp::uart::Error> {
    tx.write(packet).await?;
    // Wait for the response to be sent so we don't stomp on it
    embassy_time::Timer::after_millis(5).await;
    Ok(())
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let peripherals = embassy_rp::init(Default::default());

    let mut servo_tx_config = embassy_rp::uart::Config::default();
    servo_tx_config.baudrate = 57600;
    let mut tx_to_servo = embassy_rp::uart::UartTx::new(
        peripherals.UART0,
        peripherals.PIN_2,
        peripherals.DMA_CH0,
        servo_tx_config,
    );

    // Create embassy-usb Config
    let mut config = Config::new(0xf569, 0x0001);
    config.manufacturer = Some("Steph");
    config.product = Some("Desk Pal");
    config.serial_number = Some("00000001");
    config.max_power = 250;
    config.max_packet_size_0 = MAX_CDC_PACKET_SIZE as u8;

    let driver = UsbDriver::new(peripherals.USB, Irqs);
    let mut cdc_state = SerialState::new();
    let mut speaker_state = SpeakerState::new();

    let mut config_descriptor = [0; 256];
    let mut bos_descriptor = [0; 256];
    let mut control_buf = [0; 64];
    let mut msos_descriptor = [0; 256];
    let mut builder = Builder::new(
        driver,
        config,
        &mut config_descriptor,
        &mut bos_descriptor,
        &mut msos_descriptor,
        &mut control_buf,
    );

    let mut cdc_class: CdcAcmClass<'_, UsbDriver<'_, USB>> =
        CdcAcmClass::new(&mut builder, &mut cdc_state, MAX_CDC_PACKET_SIZE as u16);

    let (mut stream, _feedback, _control_monitor) = Speaker::new(
        &mut builder,
        &mut speaker_state,
        MAX_AUDIO_PACKET_SIZE as u16,
        embassy_usb::class::uac1::SampleWidth::Width2Byte, // 16 bit
        &[AUDIO_SAMPLE_RATE],                              // Sample rate
        &[embassy_usb::class::uac1::Channel::CenterFront], // Channels
        FeedbackRefresh::Period4Frames,
    );

    // Build the builder.
    let mut usb: embassy_usb::UsbDevice<'_, UsbDriver<'_, USB>> = builder.build();

    // Run the USB device.
    let usb_fut = usb.run();

    let cdc_future = async {
        let packet_maker = dynamixel::PacketMaker::new(SERVO_ID);

        match send_serial_command(
            &mut tx_to_servo,
            &packet_maker.set_max_position(float_to_servo_range(t_rex_angle_mapping(1.0).unwrap())),
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                defmt::error!("Failed to set max position: {}", e);
            }
        };
        match send_serial_command(
            &mut tx_to_servo,
            &packet_maker.set_min_position(float_to_servo_range(t_rex_angle_mapping(0.0).unwrap())),
        )
        .await
        {
            Ok(_) => {}
            Err(e) => {
                defmt::error!("Failed to set min position: {}", e);
            }
        };

        // Enable torque before entering command loop
        let torque_enable_packet = packet_maker.torque_state(true);
        match send_serial_command(&mut tx_to_servo, &torque_enable_packet).await {
            Ok(_) => {}
            Err(e) => {
                defmt::error!("Failed to enable torque: {}", e);
            }
        };

        // Turn on the servo LED
        match send_serial_command(&mut tx_to_servo, &packet_maker.led_packet(true)).await {
            Ok(_) => {}
            Err(e) => {
                defmt::error!("Failed to turn on LED: {}", e);
            }
        };

        // Enable LED channel
        let _ = embassy_rp::gpio::Output::new(peripherals.PIN_19, embassy_rp::gpio::Level::High);

        loop {
            cdc_class.wait_connection().await;
            info!("Serial connected");
            let _ = process_commands(&mut cdc_class, &mut tx_to_servo).await;
            info!("Serial disconnected");
        }
    };

    let audio_future = async {
        let bit_clock_pin = peripherals.PIN_5;
        let left_right_clock_pin = peripherals.PIN_6;
        let data_pin = peripherals.PIN_7;
        // Setup pio state machine for i2s output
        let Pio {
            mut common, sm0, ..
        } = Pio::new(peripherals.PIO0, Irqs);

        let program = PioI2sOutProgram::new(&mut common);
        let mut i2s = PioI2sOut::new(
            &mut common,
            sm0,
            peripherals.DMA_CH1,
            data_pin,
            bit_clock_pin,
            left_right_clock_pin,
            AUDIO_SAMPLE_RATE, // 48 kHz
            AUDIO_BIT_DEPTH,   // 16 bits
            1,                 // 1 channel (mono)
            &program,
        );

        let mut audio_packet: [u8; MAX_AUDIO_PACKET_SIZE] = [0u8; MAX_AUDIO_PACKET_SIZE];
        let mut audio_data: [u32; MAX_AUDIO_PACKET_SIZE / 4] = [0u32; MAX_AUDIO_PACKET_SIZE / 4];

        stream.wait_connection().await;
        defmt::info!("Audio stream connected");

        loop {
            let packet_size = match stream.read_packet(&mut audio_packet).await {
                Ok(size) if size == 0 => continue, // No data read, continue to next iteration
                Ok(size) => size,
                Err(e) => {
                    defmt::error!("Failed to read audio packet: {}", e);
                    continue;
                }
            };

            (&audio_packet[..packet_size])
                .chunks_exact(4)
                .enumerate()
                .for_each(|(i, chunk)| {
                    let value = u32::from_le_bytes(chunk.try_into().unwrap());
                    audio_data[i] = value;
                });
            i2s.write(&mut audio_data[..packet_size / 4]).await;
        }
    };

    embassy_futures::join::join3(usb_fut, cdc_future, audio_future).await;
}

enum UsbSerialError {
    /// Client disconnected
    Disconnected,
}

impl From<embassy_usb::driver::EndpointError> for UsbSerialError {
    fn from(val: embassy_usb::driver::EndpointError) -> Self {
        match val {
            embassy_usb::driver::EndpointError::BufferOverflow => panic!("Buffer overflow"),
            embassy_usb::driver::EndpointError::Disabled => UsbSerialError::Disconnected,
        }
    }
}

async fn process_commands<'d, T: UsbInstance + 'd>(
    class: &mut CdcAcmClass<'d, UsbDriver<'d, T>>,
    tx_to_servo: &mut embassy_rp::uart::UartTx<'d, embassy_rp::uart::Async>,
) -> Result<(), UsbSerialError> {
    let packet_maker = dynamixel::PacketMaker::new(SERVO_ID);

    let mut buffer = [0; 32];
    loop {
        let n: usize = class.read_packet(&mut buffer).await?;
        if n == 0 {
            // No data read, continue to next iteration
            continue;
        }

        let command = buffer[n - 1];
        // Map command (u8 0-255) to ratio 0.0-1.0
        let ratio = (command as f32) / 255.0;
        let angle = match t_rex_angle_mapping(ratio) {
            Ok(a) => a,
            Err(e) => {
                defmt::error!("Invalid ratio: {}", e);
                continue;
            }
        };
        let position = float_to_servo_range(angle);
        defmt::info!(
            "Received commanded position {} setting servo to position: {}",
            command,
            position
        );

        // Build and send DYNAMIXEL Write packet for Goal Position (0x74, 4 bytes)
        let packet: heapless::Vec<u8, 32> = packet_maker.set_position(position);
        match send_serial_command(tx_to_servo, &packet).await {
            Ok(_) => {}
            Err(e) => {
                defmt::error!("Failed to send packet: {}", e);
            }
        };
    }
}
