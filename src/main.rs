mod printer;

use std::io::Cursor;
use std::sync::mpsc::{SyncSender, TrySendError};
use std::time::Duration;
use axum::extract::{DefaultBodyLimit, Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{ErrorResponse, IntoResponse, Response};
use axum::{Json, Router};
use axum::routing::{get, post};

use serde::{Deserialize, Serialize};
use garde::{Validate};
use image::codecs::jpeg::JpegDecoder;
use image::ImageDecoder;
use rust_embed::Embed;
use tokio::sync::oneshot;
use crate::printer::{start_printer_worker, PrintJob, PrinterCommand, StatusJob, SubmissionImage};

const MAX_IMAGE_DIMENSION: u32 = 1024;
const MAX_IMAGE_BYTES: usize = 8 * 1024 * 1024;
const  MAX_SUBMISSION_BYTES: usize = 4 * 1024;
const MAX_REQUEST_BYTES: usize = 9 * 1024 * 1024;

#[derive(Embed)]
#[folder = "web/"]
struct WebAssets;

#[derive(Serialize)]
struct ApiErrorBody {
    ok: bool,
    error: String,
}

#[derive(Debug, Deserialize, Validate)]
#[serde(deny_unknown_fields)]
struct SubmissionPayload {
    #[garde(length(chars, min=1, max=500))]
    message: String,
    #[garde(length(chars, min=1, max=50))]
    author: String,
}

#[derive(Serialize)]
struct SuccessResponse {
    ok: bool,
    message: &'static str,
}

#[derive(Clone)]
struct AppState {
    printer_tx: SyncSender<PrinterCommand>
}

#[tokio::main]
async fn main() {
    let state = AppState {
        printer_tx: start_printer_worker(),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/api/print", post(print_submission).layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES)))
        .route("/api/printer/status", get(printer_status))
        .route("/{*path}", get(asset))
        .with_state(state);

    println!("Starting quotewall on 0.0.0.0:3000");

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000").await.unwrap();
    axum::serve(listener, app).await.unwrap();
}

async fn print_submission(State(state): State<AppState>, multipart: Multipart) -> Result<Response, ApiError> {
    let (submission, image) = parse_submission(multipart).await?;
    let (reply_tx, reply_rx) = oneshot::channel();
    let job = PrintJob {
        submission,
        image,
        reply: reply_tx,
    };

    match state.printer_tx.try_send(PrinterCommand::Print(job)) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            return Err(service_unavailable("The printer queue is full"));
        }
        Err(TrySendError::Disconnected(_)) => {
            return Err(service_unavailable("The printer worker is unavailable"));
        }
    }

    let print_result = tokio::time::timeout(Duration::from_secs(20), reply_rx)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "The printer did not respond in time",
            )
        })?
        .map_err(|_| service_unavailable("The printer worker stopped unexpectedly"))?;

    print_result.map_err(internal_error)?;

    Ok(Json(SuccessResponse {
        ok: true,
        message: "Your submission was printed.",
    }).into_response())
}

async fn printer_status(State(state): State<AppState>) -> Result<Response, ApiError> {
    let (reply_tx, reply_rx) = oneshot::channel();
    let command = PrinterCommand::Status(StatusJob { reply: reply_tx });

    match state.printer_tx.try_send(command) {
        Ok(()) => {}
        Err(TrySendError::Full(_)) => {
            return Err(service_unavailable("The printer queue is full"));
        }
        Err(TrySendError::Disconnected(_)) => {
            return Err(service_unavailable("The printer worker is unavailable"));
        }
    }

    let status = tokio::time::timeout(Duration::from_secs(5), reply_rx)
        .await
        .map_err(|_| {
            api_error(
                StatusCode::GATEWAY_TIMEOUT,
                "The printer status check timed out",
            )
        })?
        .map_err(|_| service_unavailable("The printer worker stopped unexpectedly"))?;

    Ok(Json(status).into_response())
}


async fn parse_submission(mut multipart: Multipart) -> Result<(SubmissionPayload, Option<SubmissionImage>), ApiError> {
    let mut submission = None;
    let mut image = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| bad_request(format!("Invalid multipart data: {error}")))?
    {
        let Some(name) = field.name().map(str::to_owned) else {
            continue;
        };

        match name.as_str() {
            "submission" => {
                reject_duplicate(&submission, "submission")?;

                // if field.content_type() != Some("application/json") && field.content_type() != Some("text/plain") {
                //     return Err(bad_request("The submission part must use application/json"));
                // }

                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| bad_request("The submission JSON could not be read"))?;

                if bytes.len() > MAX_SUBMISSION_BYTES {
                    return Err(api_error(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "The submission JSON is too large",
                    ));
                }

                let parsed: SubmissionPayload = serde_json::from_slice(&bytes)
                    .map_err(|error| bad_request(format!("Invalid submission JSON: {error}")))?;

                parsed
                    .validate()
                    .map_err(|report| bad_request(format!("Invalid submission: {report}")))?;

                submission = Some(parsed);
            }
            "image" => {
                reject_duplicate(&image, "image")?;

                if field.content_type() != Some("image/jpeg") {
                    return Err(bad_request("The image must be a JPEG"));
                }

                let bytes = field
                    .bytes()
                    .await
                    .map_err(|_| bad_request("The image could not be read"))?;

                if bytes.is_empty() {
                    return Err(bad_request("The image is empty"));
                }

                if bytes.len() > MAX_IMAGE_BYTES {
                    return Err(api_error(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "The JPEG must be no larger than 8 MB",
                    ));
                }

                let owned_bytes = validate_jpeg(bytes.to_vec()).await?;

                image = Some(SubmissionImage {
                    bytes: owned_bytes,
                });
            }
            _ => return Err(bad_request(format!("Unknown field: {name}"))),
        }
    }

    let submission = submission.ok_or_else(|| bad_request("The submission part is required"))?;

    Ok((submission, image))
}

fn reject_duplicate<T>(
    value: &Option<T>,
    name: &str,
) -> Result<(), ApiError> {
    if value.is_some() {
        return Err(bad_request(format!("Duplicate field: {name}")));
    }

    Ok(())
}

async fn validate_jpeg(bytes: Vec<u8>) -> Result<Vec<u8>, ApiError> {
    tokio::task::spawn_blocking(move || {
        let decoder = JpegDecoder::new(Cursor::new(bytes.as_slice()))
            .map_err(|_| bad_request("The image is not a valid JPEG"))?;
        let (width, height) = decoder.dimensions();

        if width == 0 || height == 0 {
            return Err(bad_request("The JPEG has invalid dimensions"));
        }

        if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
            return Err(bad_request(format!(
                "The JPEG must fit within {MAX_IMAGE_DIMENSION}x{MAX_IMAGE_DIMENSION} pixels"
            )));
        }

        let decoded_size = usize::try_from(decoder.total_bytes())
            .map_err(|_| bad_request("The JPEG is too large to decode"))?;
        let mut decoded = vec![0; decoded_size];

        decoder
            .read_image(&mut decoded)
            .map_err(|_| bad_request("The image is not a valid JPEG"))?;

        Ok(bytes)
    })
        .await
        .map_err(|_| internal_error("The image validator stopped unexpectedly"))?
}

async fn index() -> Response {
    embedded_file("index.html")
}

async fn asset(Path(path): Path<String>) -> Response {
    embedded_file(&path)
}

fn embedded_file(path: &str) -> Response {
    if path.contains("..") || path.starts_with('/') {
        return StatusCode::NOT_FOUND.into_response();
    }

    match WebAssets::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            (
                [(header::CONTENT_TYPE, mime.as_ref())],
                file.data.into_owned(),
            )
                .into_response()
        }
        None => StatusCode::NOT_FOUND.into_response(),
    }
}


type ApiError = ErrorResponse;

fn api_error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (
        status,
        Json(ApiErrorBody {
            ok: false,
            error: message.into(),
        }),
    )
        .into()
}

fn bad_request(message: impl Into<String>) -> ApiError {
    api_error(StatusCode::BAD_REQUEST, message)
}

fn service_unavailable(message: impl Into<String>) -> ApiError {
    api_error(StatusCode::SERVICE_UNAVAILABLE, message)
}

fn internal_error(message: impl Into<String>) -> ApiError {
    api_error(StatusCode::INTERNAL_SERVER_ERROR, message)
}
