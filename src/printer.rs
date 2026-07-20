use crate::printer::SubmissionType::Wisdom;
use crate::SubmissionPayload;
use chrono::Utc;
use chrono_tz::America::Chicago;
use escpos::driver::{Driver, FileDriver};
use escpos::printer::Printer;
use escpos::printer_options::PrinterOptions;
use escpos::ui::line::{LineBuilder, LineStyle};
use escpos::utils::{JustifyMode, Protocol, RealTimeStatusRequest, RealTimeStatusResponse};
use escpos::errors::Result as EscposResult;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage, ImageFormat};
use serde::{Deserialize, Serialize};
use std::cmp::PartialEq;
use std::io::Cursor;
use std::path::Path;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::thread;
use std::time::Duration;
use tokio::sync::oneshot;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum  SubmissionType {
    Wisdom,
    Quote,
}

pub struct SubmissionImage {
    pub bytes: Vec<u8>,
}

pub struct PrintJob {
    pub submission: SubmissionPayload,
    pub image: Option<SubmissionImage>,
    pub reply: oneshot::Sender<Result<(), String>>
}

pub struct StatusJob {
    pub reply: oneshot::Sender<PrinterAvailability>
}

#[derive(Serialize)]
pub struct PrinterAvailability {
    pub available: bool,
    pub paper_present: bool,
    pub paper_low: bool,
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

                        let status_result = match connection.as_mut() {
                            Ok((driver, printer)) => check_printer(driver, printer),
                            Err(error) => Err(error.clone()),
                        };

                        let status = match status_result {
                            Ok(status) => status,
                            Err(error) => {
                                connection = Err(error.clone());
                                PrinterAvailability {
                                    available: false,
                                    paper_present: false,
                                    paper_low: false,
                                    message: error,
                                }
                            }
                        };
                        let _ = job.reply.send(status);
                    }
                }
            }
        })
        .expect("failed to start printer worker");

    printer_tx
}

fn check_printer(
    driver: &FileDriver,
    printer: &mut Printer<FileDriver>,
) -> Result<PrinterAvailability, String> {
    printer
        .real_time_status(RealTimeStatusRequest::Printer)
        .and_then(|printer| printer.real_time_status(RealTimeStatusRequest::RollPaperSensor))
        .and_then(Printer::send_status)
        .map_err(|error| error.to_string())?;

    let mut response = [0_u8; 2];
    let bytes_read = driver
        .read(&mut response)
        .map_err(|error| error.to_string())?;

    if bytes_read != response.len() {
        return Err(format!(
            "Incomplete printer status response: expected 2 bytes, received {bytes_read}"
        ));
    }

    let printer_status = RealTimeStatusResponse::parse(RealTimeStatusRequest::Printer, response[0])
        .map_err(|error| error.to_string())?;
    let paper_status =
        RealTimeStatusResponse::parse(RealTimeStatusRequest::RollPaperSensor, response[1])
            .map_err(|error| error.to_string())?;

    let available = printer_status
        .get(&RealTimeStatusResponse::Online)
        .copied()
        .unwrap_or(false);
    let paper_present = paper_status
        .get(&RealTimeStatusResponse::RollPaperEndSensorPaperPresent)
        .copied()
        .unwrap_or(false);
    let paper_adequate = paper_status
        .get(&RealTimeStatusResponse::RollPaperNearEndSensorPaperAdequate)
        .copied()
        .unwrap_or(false);
    let paper_low = paper_present && !paper_adequate;

    let message = if !available {
        "Printer is offline."
    } else if !paper_present {
        "Printer is out of paper."
    } else if paper_low {
        "Printer is available, but the paper roll is running low."
    } else {
        "Printer is available."
    };

    Ok(PrinterAvailability {
        available,
        paper_present,
        paper_low,
        message: message.to_owned(),
    })
}

fn print_job(printer: &mut Printer<FileDriver>, job: &PrintJob) -> EscposResult<()> {
    println!("{}", format!("Incoming print job: {} {:?}", job.submission.message, job.submission.author));
    let title = if job.submission.r#type == Wisdom {
        String::from("Words of Wisdom")
    } else {
        String::from("Quote")
    };

    let date_time = Utc::now()
        .with_timezone(&Chicago)
        .format("%m/%d/%Y %I:%M %p")
        .to_string();

    printer.init()?
        .smoothing(true)?
        .justify(JustifyMode::CENTER)?
        .size(3,2)?
        .writeln(&*title)?
        .reset_size()?
        .reverse(false)?
        .bold(false)?
        .reset_size()?
        .writeln(&*date_time)?
        .feed()?
        .draw_line(LineBuilder::new().style(LineStyle::Simple).build())?;


    printer.feed()?
        .writeln(&*format!("\"{}\"", job.submission.message))?
        .feed()?;

    if let Some(author) = &job.submission.author {
        printer.justify(JustifyMode::RIGHT)?
            .bold(true)?
            .writeln(&*format!("- {}", author))?;
    }

    if let Some(image) = &job.image {
        let dithered = prepare_receipt_image(&image.bytes)?;
        printer.justify(JustifyMode::CENTER)?
            .feed()?
            .bit_image_from_bytes(&dithered)?;
    }

        printer.print_cut()?;
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