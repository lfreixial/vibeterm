//! Turn desktop images and files into paths terminal applications can consume.
use anyhow::{Context, Result, ensure};
use arboard::ImageData;
use std::path::Path;

/// Quote paths for shells, without permitting the paste filter to change a filename.
pub fn path_text(path: &Path) -> Result<String> {
    let absolute = std::path::absolute(path)?;
    let text = absolute.to_str().context("File path is not valid UTF-8")?;
    ensure!(
        !text.chars().any(char::is_control),
        "File path contains control characters"
    );
    let quoted = if text
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || "/._-".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    };
    // Separate successive drops, without submitting the terminal's input.
    Ok(format!("{quoted} "))
}

/// Keep the file after pasting: agents may read it asynchronously, even after exit.
pub fn save_image(image: ImageData<'_>) -> Result<String> {
    let width = u32::try_from(image.width).context("Image is too wide")?;
    let height = u32::try_from(image.height).context("Image is too tall")?;
    let expected = image
        .width
        .checked_mul(image.height)
        .and_then(|n| n.checked_mul(4));
    ensure!(
        width > 0 && height > 0 && expected == Some(image.bytes.len()),
        "Invalid clipboard image dimensions"
    );
    ensure!(
        image.bytes.len() <= 256 * 1024 * 1024,
        "Clipboard image exceeds 256 MiB"
    );
    let mut file = tempfile::Builder::new()
        .prefix("vibeterm-image-")
        .suffix(".png")
        .tempfile()?;
    {
        let mut encoder = png::Encoder::new(file.as_file_mut(), width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header()?;
        writer.write_image_data(&image.bytes)?;
        writer.finish()?;
    }
    let text = path_text(file.path())?;
    file.keep().context("Cannot retain clipboard image")?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{borrow::Cow, os::unix::fs::PermissionsExt};

    #[test]
    fn paths_are_quoted_and_never_submit_commands() {
        assert_eq!(
            path_text(Path::new("/tmp/image.png")).unwrap(),
            "/tmp/image.png "
        );
        assert_eq!(
            path_text(Path::new("/tmp/a '$(touch nope).png")).unwrap(),
            "'/tmp/a '\\''$(touch nope).png' "
        );
        assert!(path_text(Path::new("/tmp/a\nb.png")).is_err());
        assert!(path_text(Path::new("/tmp/a\x1bb.png")).is_err());
    }

    #[test]
    fn clipboard_pixels_round_trip_as_private_png() {
        let pixels = vec![255, 0, 42, 255, 0, 255, 20, 128];
        let text = save_image(ImageData {
            width: 2,
            height: 1,
            bytes: Cow::Borrowed(&pixels),
        })
        .unwrap();
        let path = Path::new(text.trim_end());
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        let mut reader =
            png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()))
                .read_info()
                .unwrap();
        let mut decoded = vec![0; reader.output_buffer_size().unwrap()];
        let info = reader.next_frame(&mut decoded).unwrap();
        assert_eq!((info.width, info.height), (2, 1));
        assert_eq!(decoded, pixels);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn malformed_image_is_rejected() {
        assert!(
            save_image(ImageData {
                width: usize::MAX,
                height: 2,
                bytes: Cow::Borrowed(&[])
            })
            .is_err()
        );
    }
}
