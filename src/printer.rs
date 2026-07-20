use std::cmp::PartialEq;
use std::io::{Cursor, Read};
use std::path::Path;
use crate::SubmissionPayload;
use serde::Deserialize;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::Duration;
use escpos::driver::FileDriver;
use escpos::printer::Printer;
use escpos::printer_options::PrinterOptions;
use escpos::utils::{JustifyMode, Protocol, UnderlineMode};
use tokio::sync::oneshot;
use escpos::{
    errors::Result as EscposResult
};
use escpos::ui::line::{Line, LineBuilder, LineStyle};
use image::{DynamicImage, GrayImage, ImageFormat};
use image::imageops::FilterType;
use crate::printer::SubmissionType::Wisdom;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum  SubmissionType {
    Wisdom,
    Quote,
}

pub struct SubmissionImage {
    pub bytes: Vec<u8>,
    pub filename: Option<String>,
}

pub struct PrintJob {
    pub submission: SubmissionPayload,
    pub image: Option<SubmissionImage>,
    pub reply: oneshot::Sender<Result<(), String>>
}

pub struct StatusJob {
    pub reply: oneshot::Sender<PrinterAvailability>
}

pub struct PrinterAvailability {
    pub available: bool,
    pub message: String,
}

pub enum PrinterCommand {
    Print(PrintJob),
    Status(StatusJob),
}

pub fn start_printer_worker() -> SyncSender<PrinterCommand> {
    let (printer_tx, printer_rx) = mpsc::sync_channel::<PrinterCommand>(16);

    thread::Builder::new()
        .name("receipt-printer".to_owned())
        .spawn(move || {
            let path = Path::new("/dev/usb/lp1");
            let connect = || {
                const ATTEMPTS: usize = 3;
                const RETRY_DELAY: Duration = Duration::from_millis(250);

                let mut last_error = "Printer connection failed".to_owned();

                for attempt in 1..=ATTEMPTS {
                    match FileDriver::open(path) {
                        Ok(driver) => {
                            let printer = Printer::new(
                                driver.clone(),
                                Protocol::default(),
                                Some(PrinterOptions::default()),
                            );

                            // Retain both objects. Printer owns a cloned
                            // driver, while the original driver can later be
                            // used for status reads.
                            return Ok((driver, printer));
                        }
                        Err(error) => {
                            last_error = error.to_string();

                            if attempt < ATTEMPTS {
                                thread::sleep(RETRY_DELAY * attempt as u32);
                            }
                        }
                    }
                }

                Err(last_error)
            };

            let mut connection = connect();


            while let Ok(command) = printer_rx.recv() {
                match command {
                    PrinterCommand::Print(job) => {
                        if connection.is_err() {
                            connection = connect();
                        }

                        let result = match connection.as_mut() {
                            Ok((_driver, printer)) => {
                                print_job(printer, &job).map_err(|error| error.to_string())
                            }
                            Err(error) => Err(error.clone()),
                        };

                        // When actual printer I/O replaces the placeholder
                        // above, any resulting error will discard the retained
                        // connection. The next command will attempt to reopen
                        // the device instead of reusing a broken handle.
                        if let Err(error) = &result {
                            connection = Err(error.clone());
                        }

                        let _ = job.reply.send(result);
                    }
                    PrinterCommand::Status(job) => {
                        // Drop the old file handle before attempting to reopen
                        // the USB device. This catches the common power-off
                        // case where /dev/usb/lp1 disappears or refuses open.
                        drop(std::mem::replace(
                            &mut connection,
                            Err("Refreshing printer connection".to_owned()),
                        ));
                        connection = connect();

                        let status = match connection.as_mut() {
                            Ok((_driver, printer)) => check_printer(printer),
                            Err(error) => PrinterAvailability {
                                available: false,
                                message: error.clone(),
                            },
                        };
                        let _ = job.reply.send(status);
                    }
                }
            }
        })
        .expect("failed to start printer worker");

    printer_tx
}

fn check_printer(printer: &mut Printer<FileDriver>) -> PrinterAvailability {
    // A real ESC/POS status query will use `printer` here later. For now,
    // reaching this function means the device was successfully reopened.
    PrinterAvailability {
        available: true,
        message: "Printer is available.".to_owned(),
    }
}

fn print_job(printer: &mut Printer<FileDriver>, job: &PrintJob) -> EscposResult<()> {
    // Actual ESC/POS output will use `printer` and `job` here later.
    let title = if job.submission.r#type == Wisdom {
        String::from("Words of Wisdom")
    } else {
        String::from("Quote")
    };

    printer.init()?
        .smoothing(true)?
        .justify(JustifyMode::CENTER)?
        .size(3,2)?
        .bold(true)?
        .reverse(true)?
        .writeln(&*title)?
        .reset_size()?
        .reverse(false)?
        .bold(false)?
        .feed()?
        .draw_line(LineBuilder::new().style(LineStyle::Simple).offset(4).build())?;

    if let Some(image) = &job.image {
        let dithered = prepare_receipt_image(&image.bytes)?;
        printer.bit_image_from_bytes(&dithered)?;
    }

        // .writeln("Bold underline")?
        // .justify(JustifyMode::CENTER)?
        // .reverse(true)?
        // .bold(false)?
        // .writeln("Hello world - Reverse")?
        // .feed()?
        // .justify(JustifyMode::RIGHT)?
        // .reverse(false)?
        // .underline(UnderlineMode::None)?
        // .size(2, 3)?
        // .writeln("Hello world - Normal")?
        printer.print_cut()?;
    println!("--- RECEIPT ---");
    Ok(())
}

fn prepare_receipt_image(bytes: &[u8]) -> image::ImageResult<Vec<u8>> {
    const PRINT_WIDTH: u32 = 384;

    let source = image::load_from_memory(bytes)?;
    let resized = source.resize(PRINT_WIDTH, u32::MAX, FilterType::Lanczos3);
    let grayscale = resized.to_luma8();
    let contrasted = image::imageops::contrast(&grayscale, 15.0);
    let sharpened = image::imageops::unsharpen(&contrasted, 1.0, 1);
    let dithered = floyd_steinberg(sharpened);

    let mut png = Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(dithered)
        .write_to(&mut png, ImageFormat::Png)?;

    Ok(png.into_inner())
}

fn floyd_steinberg(image: GrayImage) -> GrayImage {
    let (width, height) = image.dimensions();
    let mut luminance: Vec<f32> = image.pixels().map(|pixel| f32::from(pixel[0])).collect();
    let mut output = GrayImage::new(width, height);

    for y in 0..height {
        for x in 0..width {
            let index = (y * width + x) as usize;
            let old = luminance[index].clamp(0.0, 255.0);
            let new = if old < 128.0 { 0 } else { 255 };
            let error = old - f32::from(new);
            output.put_pixel(x, y, image::Luma([new]));

            if x + 1 < width {
                luminance[index + 1] += error * 7.0 / 16.0;
            }
            if y + 1 < height {
                if x > 0 {
                    luminance[(index + width as usize) - 1] += error * 3.0 / 16.0;
                }
                luminance[index + width as usize] += error * 5.0 / 16.0;
                if x + 1 < width {
                    luminance[index + width as usize + 1] += error / 16.0;
                }
            }
        }
    }

    output
}