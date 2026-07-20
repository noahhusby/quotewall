use crate::SubmissionPayload;
use serde::Deserialize;
use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::thread;
use tokio::sync::oneshot;

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
            while let Ok(command) = printer_rx.recv() {
                match command {
                    PrinterCommand::Print(job) => {
                        let result = print_job(&job);
                        let _ = job.reply.send(result);
                    }
                    PrinterCommand::Status(job) => {
                        let status = check_printer();
                        let _ = job.reply.send(status);
                    }
                }
            }
        })
        .expect("failed to start printer worker");

    printer_tx
}

fn check_printer() -> PrinterAvailability {
    PrinterAvailability {
        available: true,
        message: "Printer is available.".to_owned(),
    }
}


fn print_job(job: &PrintJob) -> Result<(), String> {
    println!("--- RECEIPT ---");
    Ok(())
}