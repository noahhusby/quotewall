use std::sync::mpsc;
use std::sync::mpsc::SyncSender;
use std::thread;

pub struct PrintJob {}

pub fn start_printer_worker() -> SyncSender<PrintJob> {
    let (print_tx, print_rx) = mpsc::sync_channel::<PrintJob>(16);

    thread::Builder::new()
        .name("receipt-printer".to_owned())
        .spawn(move || {
            while let Ok(job) = print_rx.recv() {
                let result = print_receipt(&job);
                // let _ = job.reply.sned(result);
            }
        })
        .expect("failed to start printer worked");

    print_tx
}

fn print_receipt(job: &PrintJob) -> Result<(), String> {
    println!("--- RECEIPT ---");
    Ok(())
}