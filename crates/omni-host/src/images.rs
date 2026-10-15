//! Button images imported by the user: downscaled PNGs, content-addressed,
//! stored in `<data_dir>/images/<id>.png` and served at `/img/<id>`.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use anyhow::Context;
use sha2::{Digest, Sha256};

/// Big enough for a crisp button on a 3x density screen.
const MAX_SIZE: u32 = 288;

pub fn import(data_dir: &Path, source: &Path) -> anyhow::Result<String> {
    let image = image::open(source).with_context(|| format!("can't read image {}", source.display()))?;
    let image =
        if image.width() > MAX_SIZE || image.height() > MAX_SIZE { image.thumbnail(MAX_SIZE, MAX_SIZE) } else { image };
    let mut png = Vec::new();
    image.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)?;
    let id = hex::encode(&Sha256::digest(&png)[..10]);
    let dir = data_dir.join("images");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{id}.png"));
    if !path.exists() {
        std::fs::write(&path, &png)?;
    }
    Ok(id)
}

/// File for an image id, rejecting anything that isn't a plain hex id.
pub fn path(data_dir: &Path, id: &str) -> Option<PathBuf> {
    let valid = !id.is_empty() && id.len() <= 64 && id.bytes().all(|b| b.is_ascii_hexdigit());
    valid.then(|| data_dir.join("images").join(format!("{id}.png")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_downscales_and_dedupes() {
        let dir = std::env::temp_dir().join(format!("omni-img-{}", omni_core::new_id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("big.png");
        image::RgbaImage::from_pixel(1000, 500, image::Rgba([255, 0, 0, 255])).save(&src).unwrap();

        let id = import(&dir, &src).unwrap();
        assert_eq!(import(&dir, &src).unwrap(), id);
        let stored = image::open(path(&dir, &id).unwrap()).unwrap();
        assert_eq!((stored.width(), stored.height()), (288, 144));
        assert!(path(&dir, "../config").is_none());
        std::fs::remove_dir_all(dir).ok();
    }
}
