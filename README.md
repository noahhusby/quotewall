# Quotewall
Sometimes a real good quote needs remembering.

## About

Quotewall is a web app that allows my friends to memorialize quotes using a thermal printer. The program is a web app that interfaces with compatible printers using the ESC/POS protocol from Epson. 

**Webpage:**

![](/assets/webpage_example.png)

**Result:**

![](/assets/printer_example.png)

## Building

The program can be built for any platform using Rust's cargo tool.

To run the program in dev:
1. `cargo run`

To build the program for production
1. `cargo build -r`
2. Find the built app in `/target/release`

## License

This project is licensed under the `MIT License`.