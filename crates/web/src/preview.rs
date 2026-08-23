//! The preview proxy, and input on the way back.
//!
//! ## Why this is proxied rather than fetched by the browser
//!
//! Three reasons, any one of which would be enough:
//!
//! 1. **WebLinked sends no CORS headers**, so a page served by rookery cannot
//!    read `http://gfx-1:7654/api/preview` at all.
//! 2. **The token would have to go to the browser.** rookery holds each
//!    instance's `--token` so that it stays on the server; handing it to every
//!    open tab to let them fetch directly would undo that.
//! 3. **Raw BGRA is the wrong thing to put on the browser leg.** A 480x270
//!    frame is 518 KB uncompressed and about 20 KB as JPEG. rookery is
//!    normally on the show LAN with the instances; the operator's browser
//!    frequently is not.
//!
//! ## The sequence is the ETag
//!
//! WebLinked's `X-Frame-Sequence` identifies the **paint**. A static graphic
//! holds the same value indefinitely, so a conditional request collapses to a
//! `304` with no body — a wall of eight lower-thirds that nobody is touching
//! costs almost nothing to keep on screen. The saving is on the browser leg
//! only: rookery still has to fetch from the instance to learn the sequence,
//! because WebLinked has no conditional GET of its own.

use axum::extract::{Path, Query, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use rookery_core::{InputEvent, PreviewFrame, PreviewUnavailable};

use crate::error::{parse_instance_id, ApiError};
use crate::state::AppState;

#[derive(Deserialize)]
pub struct PreviewQuery {
    #[serde(default)]
    pub source: Option<String>,
    /// JPEG quality, 1..=100. The default is deliberately modest: this is a
    /// monitoring thumbnail, not a mastering path, and the difference between
    /// 60 and 90 on a 240x135 tile is invisible and doubles the bytes.
    #[serde(default = "default_quality")]
    pub quality: u8,
}

fn default_quality() -> u8 {
    60
}

pub async fn get_preview(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<PreviewQuery>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let id = parse_instance_id(&id)?;
    let instance = state
        .fleet
        .registry()
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("no instance {id}")))?;

    let client = state.fleet.client_for(&instance);
    let frame = match client.preview(query.source.as_deref()).await {
        Ok(Ok(frame)) => frame,
        // A working instance with no picture. 204 rather than 404: the
        // *instance* is fine and the browser should keep the tile rather than
        // treat it as a broken image, and the reason travels in a header so the
        // UI can say which.
        Ok(Err(reason)) => {
            let why = match reason {
                PreviewUnavailable::NotConfigured => "not-configured",
                PreviewUnavailable::NoFrameYet => "no-frame-yet",
            };
            return Ok((
                StatusCode::NO_CONTENT,
                [(
                    header::HeaderName::from_static("x-preview-unavailable"),
                    why,
                )],
            )
                .into_response());
        }
        Err(e) => return Err(ApiError::Internal(e)),
    };

    // The paint identifier, quoted as a weak-ish ETag. Includes the raster and
    // the quality because both change the bytes without changing the paint —
    // without them, dropping the factor while a client held a cached tile would
    // serve a 304 for a picture that is now a different size.
    let etag = format!(
        "\"{}-{}x{}-q{}\"",
        frame.sequence, frame.width, frame.height, query.quality
    );
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == etag)
    {
        return Ok((StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response());
    }

    let jpeg = encode_jpeg(&frame, query.quality.clamp(1, 100))
        .map_err(|e| ApiError::Internal(e.context("encoding the preview as JPEG")))?;

    Ok((
        [
            (header::CONTENT_TYPE, "image/jpeg".to_string()),
            (header::ETAG, etag),
            // Never let a proxy or the browser hold one of these: the whole
            // point is that the next request shows what is on air now.
            (header::CACHE_CONTROL, "no-cache".to_string()),
            (
                header::HeaderName::from_static("x-frame-sequence"),
                frame.sequence.to_string(),
            ),
        ],
        jpeg,
    )
        .into_response())
}

/// BGRA to JPEG.
///
/// The channel swap is the whole job and it is the easy thing to get backwards:
/// WebLinked hands back **B, G, R, A** in that byte order, and every graphic in
/// this world is full of brand colours, so a red/blue transposition looks
/// plausible rather than obviously broken. `tests::bgra_channel_order` pins it.
fn encode_jpeg(frame: &PreviewFrame, quality: u8) -> anyhow::Result<Vec<u8>> {
    anyhow::ensure!(frame.is_complete(), "refusing to encode a short frame");

    let mut rgb = Vec::with_capacity(frame.width as usize * frame.height as usize * 3);
    for pixel in frame.bgra.as_chunks::<4>().0 {
        rgb.push(pixel[2]); // R
        rgb.push(pixel[1]); // G
        rgb.push(pixel[0]); // B
                            // Alpha is dropped: JPEG has none, and a preview is
                            // composited for looking at rather than for keying.
    }

    let mut out = Vec::new();
    let mut encoder = jpeg_encoder::Encoder::new(&mut out, quality);
    encoder.set_progressive(false);
    encoder.encode(
        &rgb,
        frame.width as u16,
        frame.height as u16,
        jpeg_encoder::ColorType::Rgb,
    )?;
    Ok(out)
}

// -------------------------------------------------------------------- input

#[derive(Deserialize)]
pub struct InputQuery {
    #[serde(default)]
    pub source: Option<String>,
}

#[derive(Deserialize)]
pub struct InputBody {
    pub events: Vec<InputEvent>,
}

pub async fn post_input(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Query(query): Query<InputQuery>,
    Json(body): Json<InputBody>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let id = parse_instance_id(&id)?;
    let instance = state
        .fleet
        .registry()
        .get(&id)
        .ok_or_else(|| ApiError::NotFound(format!("no instance {id}")))?;

    if body.events.is_empty() {
        return Ok(Json(serde_json::json!({ "ok": true, "events": 0 })));
    }
    // A guard against a runaway client rather than against an operator: a drag
    // is tens of events, and anything sending thousands in one request is a
    // loop, not a person.
    if body.events.len() > 512 {
        return Err(ApiError::BadRequest(format!(
            "{} events in one batch; 512 is the limit",
            body.events.len()
        )));
    }

    // Logged at debug, not info: a drag is a lot of lines, and unlike a command
    // this is not something anyone audits after the show.
    tracing::debug!(
        instance = %instance.name,
        events = body.events.len(),
        actuating = body.events.iter().filter(|e| e.is_actuating()).count(),
        "input"
    );

    let client = state.fleet.client_for(&instance);
    client
        .send_input(&body.events, query.source.as_deref())
        .await?;
    Ok(Json(
        serde_json::json!({ "ok": true, "events": body.events.len() }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, b: u8, g: u8, r: u8) -> PreviewFrame {
        let mut bgra = Vec::new();
        for _ in 0..(width * height) {
            bgra.extend_from_slice(&[b, g, r, 255]);
        }
        PreviewFrame {
            width,
            height,
            sequence: 1,
            bgra,
        }
    }

    /// The one that would otherwise ship: a red graphic previewing as blue.
    /// Encode a known pure red and decode it back.
    #[test]
    fn bgra_channel_order_survives_the_encode() {
        // Pure red in BGRA is B=0, G=0, R=255.
        let frame = solid(16, 16, 0, 0, 255);
        let jpeg = encode_jpeg(&frame, 95).unwrap();

        let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(&jpeg));
        let pixels = decoder.decode().unwrap();
        let info = decoder.info().unwrap();
        assert_eq!(info.pixel_format, jpeg_decoder::PixelFormat::RGB24);

        // Sample the middle, away from any block edge.
        let middle =
            ((info.height as usize / 2) * info.width as usize + info.width as usize / 2) * 3;
        let (r, g, b) = (pixels[middle], pixels[middle + 1], pixels[middle + 2]);
        assert!(
            r > 200 && g < 60 && b < 60,
            "red went in and ({r},{g},{b}) came out — the channels are swapped"
        );
    }

    #[test]
    fn a_short_frame_is_refused_rather_than_encoded() {
        let mut frame = solid(16, 16, 0, 0, 255);
        frame.bgra.truncate(100);
        assert!(encode_jpeg(&frame, 60).is_err());
    }

    #[test]
    fn jpeg_is_dramatically_smaller_than_the_raw_frame() {
        // The reason the proxy encodes at all.
        let frame = solid(480, 270, 40, 60, 80);
        let jpeg = encode_jpeg(&frame, 60).unwrap();
        assert!(
            jpeg.len() * 10 < frame.bgra.len(),
            "{} bytes of JPEG against {} raw is not worth the CPU",
            jpeg.len(),
            frame.bgra.len()
        );
    }
}
