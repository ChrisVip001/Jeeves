//! Per-turn capability line derivation (WP5).
//!
//! The platform may assign different model profiles per capability line. Which line a turn
//! needs comes from what the person actually sent: a video file asks for video understanding,
//! an image asks for vision. This module keeps the derivation pure so it can be pinned by
//! tests without a live Runtime. It never decides quota or permission; it only names the need.

use std::path::Path;

use jeeves_protocol::TurnAttachment;

/// Mainstream video extensions. Case-insensitive; confirmed at implementation time per the
/// design document. A file with one of these is a video-understanding request, not an image.
const VIDEO_EXTENSIONS: [&str; 8] = ["mp4", "mov", "webm", "mkv", "avi", "m4v", "wmv", "flv"];

/// Whether any attachment is an image. Used for the vision gate independently of the
/// chosen capability line (a turn may carry both a video and an image).
pub(crate) fn has_image(attachments: Option<&[TurnAttachment]>) -> bool {
    attachments.is_some_and(|items| items.iter().any(is_image_attachment))
}

/// The capability line this turn needs, derived from its attachments only.
/// `language` when there is nothing special; video takes precedence over image.
pub(crate) fn required_line(attachments: Option<&[TurnAttachment]>) -> &'static str {
    let Some(attachments) = attachments else {
        return "language";
    };
    if attachments.iter().any(is_video_attachment) {
        "video-understanding"
    } else if attachments.iter().any(is_image_attachment) {
        "multimodal"
    } else {
        "language"
    }
}

fn is_video_attachment(attachment: &TurnAttachment) -> bool {
    let path_ext = Path::new(&attachment.path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    path_ext
        .as_deref()
        .is_some_and(|ext| VIDEO_EXTENSIONS.contains(&ext))
        || attachment
            .media_type
            .as_deref()
            .is_some_and(|hint| hint.to_ascii_lowercase().starts_with("video/"))
}

fn is_image_attachment(attachment: &TurnAttachment) -> bool {
    if is_video_attachment(attachment) {
        return false;
    }
    let path_ext = Path::new(&attachment.path)
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase);
    path_ext.as_deref().is_some_and(|ext| {
        matches!(
            ext,
            "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "heic" | "heif"
        )
    }) || attachment
        .media_type
        .as_deref()
        .is_some_and(|hint| hint.to_ascii_lowercase().starts_with("image/"))
}

/// The refusal text for a BYOK/Local profile that cannot serve the required line (decision 7).
/// Explicit and friendly; never suggests silently borrowing a platform line.
pub(crate) fn refusal_for_missing_line(line: &str) -> Option<String> {
    match line {
        "multimodal" => Some(
            "你的模型看不了图。可以换一个支持图像的模型，或切换到平台提供的多模态模型。".into(),
        ),
        "video-understanding" => Some(
            "你的模型看不了视频。可以换一个支持视频理解的模型，或切换到平台提供的视频模型。".into(),
        ),
        "video-generation" => Some("你的模型不能生成视频。请使用平台提供的视频生成模型。".into()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(path: &str, media_type: Option<&str>) -> TurnAttachment {
        TurnAttachment {
            path: path.to_string(),
            media_type: media_type.map(str::to_string),
        }
    }

    #[test]
    fn no_attachments_is_language() {
        assert_eq!(required_line(None), "language");
        assert_eq!(required_line(Some(&[])), "language");
    }

    #[test]
    fn video_extensions_win_over_images() {
        let items = [
            ("clip.MP4", None),
            ("movie.mov", None),
            ("web.webm", None),
            ("a.mkv", None),
            ("b.avi", None),
            ("c.m4v", None),
            ("d.wmv", None),
            ("e.flv", None),
            ("unknown.bin", Some("video/mp4")),
        ];
        for (path, hint) in items {
            assert_eq!(
                required_line(Some(&[attachment(path, hint)])),
                "video-understanding",
                "{path} should be recognised as video"
            );
        }
        let mixed = [attachment("photo.png", None), attachment("clip.mp4", None)];
        assert_eq!(required_line(Some(&mixed)), "video-understanding");
    }

    #[test]
    fn image_extensions_and_hints_are_multimodal() {
        for path in ["a.png", "b.JPG", "c.jpeg", "d.gif", "e.webp", "f.heic"] {
            assert_eq!(
                required_line(Some(&[attachment(path, None)])),
                "multimodal",
                "{path} should be recognised as an image"
            );
        }
        assert_eq!(
            required_line(Some(&[attachment("data.bin", Some("image/png"))])),
            "multimodal"
        );
    }

    #[test]
    fn ordinary_files_stay_language() {
        assert_eq!(
            required_line(Some(&[attachment("notes.txt", None)])),
            "language"
        );
        assert_eq!(
            required_line(Some(&[attachment("data.pdf", None)])),
            "language"
        );
    }

    #[test]
    fn refusals_name_the_missing_capability() {
        assert!(refusal_for_missing_line("language").is_none());
        let image = refusal_for_missing_line("multimodal").expect("image refusal");
        assert!(image.contains("看不了图"), "{image}");
        let video = refusal_for_missing_line("video-understanding").expect("video refusal");
        assert!(video.contains("看不了视频"), "{video}");
    }
}
