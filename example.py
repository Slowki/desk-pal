import logging
import sys
import time

import serial.tools.list_ports


def discover_device() -> str:
    """Discover a Desk Pal."""
    ports = serial.tools.list_ports.comports()
    desk_pals = [
        port.device for port in ports if port.description.strip() == "Desk Pal"
    ]

    if not desk_pals:
        sys.exit("No Desk Pal devices found. Please connect a Desk Pal and try again.")
    if len(desk_pals) > 1:
        sys.exit(
            "Multiple Desk Pal devices found. Please disconnect all but one and try again."
        )

    return desk_pals[0]


def make_command(openness: float) -> bytes:
    """Create a command to set the openness of the Desk Pal."""
    if not (0.0 <= openness <= 1.0):
        raise ValueError("Openness must be between 0.0 and 1.0")

    openness_value = int(openness * 255)

    return bytes([openness_value])


def main() -> None:
    """Main entry point."""
    device_path = discover_device()
    device = serial.Serial(port=device_path)

    openness = 0.0
    while True:
        print(f"Opening to {openness}")
        device.write(make_command(openness))
        openness += 0.5
        if openness > 1.0:
            openness = 0.0
        time.sleep(1)


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    main()
