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

#[derive(Debug, Clone, Copy, Deserialize)]
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
    printer.init()?
        .smoothing(true)?
        .bold(true)?
        .underline(UnderlineMode::Single)?
        .writeln("Bold underline")?
        .justify(JustifyMode::CENTER)?
        .reverse(true)?
        .bold(false)?
        .writeln("Hello world - Reverse")?
        .feed()?
        .justify(JustifyMode::RIGHT)?
        .reverse(false)?
        .underline(UnderlineMode::None)?
        .size(2, 3)?
        .writeln("Hello world - Normal")?
        .print_cut()?;
    println!("--- RECEIPT ---");
    Ok(())
}