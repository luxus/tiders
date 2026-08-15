//! Public cover-art helpers.
//!
//! TIDAL serves artwork from `resources.tidal.com` without authentication, so
//! front-ends can fetch album/artist covers directly. Image *ids* are UUID-like
//! strings with dashes; the CDN path replaces each dash with a slash.

use crate::error::{Error, Result};

/// Build a public cover-art URL at `size`×`size` pixels.
///
/// Valid sizes mirror TIDAL's rendition ladder: 80, 160, 320, 640, 1280.
///
/// ```
/// use tiders_core::images::cover_url;
/// assert_eq!(
///     cover_url("aaaa-bbbb-cccc", 320),
///     "https://resources.tidal.com/images/aaaa/bbbb/cccc/320x320.jpg"
/// );
/// ```
pub fn cover_url(cover_id: &str, size: u32) -> String {
    let path = cover_id.replace('-', "/");
    format!("https://resources.tidal.com/images/{path}/{size}x{size}.jpg")
}

/// Fetch the raw bytes of an image URL (JPEG for TIDAL covers).
pub async fn fetch(url: &str) -> Result<Vec<u8>> {
    let client = reqwest::Client::builder()
        .user_agent("tiders/0.1")
        .build()
        .map_err(|e| Error::other(format!("http client: {e}")))?;
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|e| Error::other(format!("cover request: {e}")))?
        .error_for_status()
        .map_err(|e| Error::other(format!("cover status: {e}")))?;
    let bytes = response
        .bytes()
        .await
        .map_err(|e| Error::other(format!("cover body: {e}")))?;
    Ok(bytes.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_url_replaces_dashes_with_slashes() {
        assert_eq!(
            cover_url("12ab-34cd-56ef", 640),
            "https://resources.tidal.com/images/12ab/34cd/56ef/640x640.jpg"
        );
    }
}
