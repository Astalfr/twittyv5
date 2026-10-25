//! Images resampled to the size they are drawn at.
//!
//! GPUI draws a bitmap with no mipmaps, so a photo shrunk to a third of its
//! size on screen looks jagged (see `state::image_scale`). [`source`] (remote
//! images) and [`asset`] (embedded ones) decode the image themselves and
//! shrink it on the CPU with a proper filter first, in GPUI's background
//! executor. [`cropped`] also cuts a media image to the part Twitter
//! suggests for the box's shape before shrinking it.

use std::{collections::HashMap, io::Cursor, sync::Arc};

use futures::AsyncReadExt as _;
use gpui_kit::{
    App, Asset, Global, ImageCacheError, ImageSource, RenderImage, SharedString, SharedUri, Window,
    http_client::{AsyncBody, HttpClient},
};
use image::{DynamicImage, Frame, ImageDecoder as _, ImageReader, imageops::FilterType};

use twitty_model::ImageRef;

use crate::state::{
    image_scale::{BoxPx, cover_size},
    media_layout::{Crop, crop_for},
};

/// An image source for `url` drawn in a `w` × `h` (logical pixels) box with
/// `ObjectFit::Cover`.
pub(crate) fn source(url: &str, w: f32, h: f32) -> ImageSource {
    resampled(Origin::Remote(SharedUri::from(url.to_owned())), w, h, None)
}

/// Like [`source`] for a media image, first cut to its suggested crop for the
/// box's shape (`media_layout::crop_for`); around its centre when it has none.
pub(crate) fn cropped(image: &ImageRef, w: f32, h: f32) -> ImageSource {
    let origin = Origin::Remote(SharedUri::from(image.url.to_string()));
    resampled(origin, w, h, crop_for(image, w, h))
}

/// An image source for the app asset at `path` (see `ui::assets`) drawn in a
/// `w` × `h` (logical pixels) box with `ObjectFit::Cover`. The asset can be
/// much larger than the box, so it stays sharp at any display scaling.
pub(crate) fn asset(path: &'static str, w: f32, h: f32) -> ImageSource {
    resampled(Origin::Asset(SharedString::new_static(path)), w, h, None)
}

fn resampled(origin: Origin, w: f32, h: f32, crop: Option<Crop>) -> ImageSource {
    ImageSource::Custom(Arc::new(move |window: &mut Window, cx: &mut App| {
        let size = BoxPx::bucket(w, h, window.scale_factor())?;
        let key = Key { origin: origin.clone(), size, crop };
        match window.use_asset::<Scaled>(&key, cx) {
            Some(result) => {
                if let Ok(image) = &result {
                    // `default_global` queues an effect on every call, and this
                    // runs every frame: write only when the copy changes.
                    let known = cx
                        .try_global::<Latest>()
                        .and_then(|l| l.0.get(&key.origin))
                        .is_some_and(|known| Arc::ptr_eq(known, image));
                    if !known {
                        cx.default_global::<Latest>().0.insert(key.origin, Arc::clone(image));
                    }
                }
                Some(result)
            }
            // A new size is still loading (the column or the scale changed):
            // keep drawing the previous copy instead of a blank box.
            None => cx.try_global::<Latest>()?.0.get(&key.origin).cloned().map(Ok),
        }
    }))
}

/// The last copy of each image that finished loading, at whatever size.
#[derive(Default)]
struct Latest(HashMap<Origin, Arc<RenderImage>>);

impl Global for Latest {}

/// Where an image's bytes come from.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum Origin {
    /// Fetched over HTTP.
    Remote(SharedUri),
    /// The app's asset source (`ui::assets`).
    Asset(SharedString),
}

/// An image at one box bucket, maybe cut to a crop first.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Key {
    pub(crate) origin: Origin,
    pub(crate) size: BoxPx,
    pub(crate) crop: Option<Crop>,
}

/// Loads a [`Key`]: fetch or load, decode, crop, shrink to cover the box.
pub(crate) enum Scaled {}

impl Asset for Scaled {
    type Source = Key;
    type Output = Result<Arc<RenderImage>, ImageCacheError>;

    fn load(
        key: Self::Source,
        cx: &mut App,
    ) -> impl Future<Output = Self::Output> + Send + 'static {
        let client = cx.http_client();
        let assets = Arc::clone(cx.asset_source());
        async move {
            let bytes = match &key.origin {
                Origin::Remote(uri) => fetch(client.as_ref(), uri).await?,
                Origin::Asset(path) => assets
                    .load(path)?
                    .ok_or_else(|| anyhow::anyhow!("no asset {path}"))?
                    .into_owned(),
            };
            decode(&bytes, key.size, key.crop)
        }
    }
}

async fn fetch(client: &dyn HttpClient, uri: &SharedUri) -> Result<Vec<u8>, ImageCacheError> {
    let mut response = client.get(uri.as_ref(), AsyncBody::empty(), true).await?;
    let mut body = Vec::new();
    response.body_mut().read_to_end(&mut body).await?;
    if !response.status().is_success() {
        return Err(ImageCacheError::BadStatus {
            uri: uri.clone(),
            status: response.status(),
            body: String::new(),
        });
    }
    Ok(body)
}

/// Decodes `bytes` (first frame only) into GPUI's BGRA format, cut to `crop`
/// and shrunk to just cover `size` when it is much larger. Mirrors GPUI's own
/// static decoding, including the EXIF orientation.
pub(crate) fn decode(
    bytes: &[u8],
    size: BoxPx,
    crop: Option<Crop>,
) -> Result<Arc<RenderImage>, ImageCacheError> {
    let mut decoder = ImageReader::new(Cursor::new(bytes)).with_guessed_format()?.into_decoder()?;
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    if let Some((x, y, w, h)) = crop.and_then(|c| c.in_pixels((image.width(), image.height()))) {
        image = image.crop_imm(x, y, w, h);
    }
    if let Some((w, h)) = cover_size((image.width(), image.height()), size) {
        // The filter widens with the shrink factor, so every source pixel
        // counts. `resize_exact` isn't generic, so it runs at `image`'s
        // opt-level even in debug builds.
        image = image.resize_exact(w, h, FilterType::CatmullRom);
    }
    let mut pixels = image.into_rgba8();
    for pixel in pixels.as_chunks_mut::<4>().0 {
        pixel.swap(0, 2);
    }
    Ok(Arc::new(RenderImage::new(vec![Frame::new(pixels)])))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui_kit::{DevicePixels, size};
    use image::{ImageFormat, Rgba, RgbaImage};

    /// A `w` × `h` PNG, red on the left half and blue on the right.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let image = RgbaImage::from_fn(w, h, |x, _| {
            if x < w / 2 { Rgba([255, 0, 0, 255]) } else { Rgba([0, 0, 255, 255]) }
        });
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    fn dims(image: &RenderImage) -> (i32, i32) {
        let s = image.size(0);
        (s.width.0, s.height.0)
    }

    #[test]
    fn a_large_image_is_shrunk_to_cover_its_box() {
        let image = decode(&png(1200, 800), BoxPx { w: 400, h: 200 }, None).unwrap();
        assert_eq!(dims(&image), (400, 267));
    }

    #[test]
    fn a_small_image_keeps_its_size() {
        let image = decode(&png(300, 200), BoxPx { w: 400, h: 200 }, None).unwrap();
        assert_eq!(dims(&image), (300, 200));
    }

    #[test]
    fn pixels_are_bgra() {
        let image = decode(&png(1200, 800), BoxPx { w: 100, h: 100 }, None).unwrap();
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(&bytes[..4], &[0, 0, 255, 255], "red, stored as BGRA");
        assert_eq!(&bytes[bytes.len() - 4..], &[255, 0, 0, 255], "blue, stored as BGRA");
    }

    #[test]
    fn a_crop_is_cut_before_shrinking() {
        // The right half of a red | blue image: all blue, at the box's size.
        let crop = Crop { x: 5_000, y: 0, w: 5_000, h: 10_000 };
        let image = decode(&png(1200, 800), BoxPx { w: 150, h: 200 }, Some(crop)).unwrap();
        assert_eq!(dims(&image), (150, 200));
        let bytes = image.as_bytes(0).unwrap();
        assert!(bytes.chunks(4).all(|p| p == [255, 0, 0, 255]), "only blue (BGRA) is left");
    }

    #[test]
    fn garbage_is_an_error_not_a_panic() {
        assert!(decode(b"not an image", BoxPx { w: 10, h: 10 }, None).is_err());
        assert!(decode(&[], BoxPx { w: 10, h: 10 }, None).is_err());
    }

    #[gpui_kit::test]
    fn an_img_loads_the_shrunk_copy_over_http(cx: &mut gpui_kit::TestAppContext) {
        let body = png(1200, 800);
        cx.update(|cx| {
            cx.set_http_client(gpui_kit::http_client::FakeHttpClient::create(move |_| {
                let body = body.clone();
                async move {
                    Ok(gpui_kit::http_client::Response::builder()
                        .status(200)
                        .body(AsyncBody::from(body))?)
                }
            }));
        });
        let uri = "https://pbs.twimg.com/media/A.jpg?name=medium";
        let (_view, cx) = cx.add_window_view(|_, _| Shown(uri));
        cx.run_until_parked();

        let scale = cx.update(|window, _| window.scale_factor());
        let key = Key {
            origin: Origin::Remote(SharedUri::from(uri)),
            size: BoxPx::bucket(200., 100., scale).unwrap(),
            crop: None,
        };
        let image = cx.update(|_, cx| cx.fetch_asset::<Scaled>(&key)).expect("loaded").unwrap();
        let (w, h) = cover_size((1200, 800), key.size).expect("shrunk");
        assert_eq!(image.size(0), size(DevicePixels(w as i32), DevicePixels(h as i32)));
    }

    #[gpui_kit::test]
    fn a_media_img_loads_its_suggested_crop(cx: &mut gpui_kit::TestAppContext) {
        let body = png(1200, 800);
        cx.update(|cx| {
            cx.set_http_client(gpui_kit::http_client::FakeHttpClient::create(move |_| {
                let body = body.clone();
                async move {
                    Ok(gpui_kit::http_client::Response::builder()
                        .status(200)
                        .body(AsyncBody::from(body))?)
                }
            }));
        });
        // A 1:2 crop in the blue right half, for a 1:2 tile.
        let url = "https://pbs.twimg.com/media/A.jpg?name=medium".parse().unwrap();
        let image = ImageRef::sized(url, Some((1200, 800))).with_crops([twitty_model::Region {
            x: 700,
            y: 0,
            w: 400,
            h: 800,
        }]);
        let (_view, cx) = cx.add_window_view({
            let image = image.clone();
            move |_, _| ShownCropped(image)
        });
        cx.run_until_parked();

        let scale = cx.update(|window, _| window.scale_factor());
        let key = Key {
            origin: Origin::Remote(SharedUri::from(image.url.to_string())),
            size: BoxPx::bucket(100., 200., scale).unwrap(),
            crop: crop_for(&image, 100., 200.),
        };
        assert!(key.crop.is_some());
        let loaded = cx.update(|_, cx| cx.fetch_asset::<Scaled>(&key)).expect("loaded").unwrap();
        let (w, h) = cover_size((400, 800), key.size).unwrap_or((400, 800));
        assert_eq!(loaded.size(0), size(DevicePixels(w as i32), DevicePixels(h as i32)));
        let bytes = loaded.as_bytes(0).unwrap();
        assert!(bytes.chunks(4).all(|p| p == [255, 0, 0, 255]), "only the blue half (BGRA)");
    }

    #[gpui_kit::test]
    fn a_missing_asset_is_an_error_not_a_panic(cx: &mut gpui_kit::TestAppContext) {
        // The test app has no assets.
        let key = Key {
            origin: Origin::Asset("brand/none.png".into()),
            size: BoxPx { w: 20, h: 20 },
            crop: None,
        };
        cx.update(|cx| cx.fetch_asset::<Scaled>(&key));
        cx.run_until_parked();
        let result = cx.update(|cx| cx.fetch_asset::<Scaled>(&key)).expect("loaded");
        assert!(result.is_err());
    }

    struct Shown(&'static str);

    impl gpui_kit::Render for Shown {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{Styled as _, px};
            gpui_kit::img(source(self.0, 200., 100.)).w(px(200.)).h(px(100.))
        }
    }

    struct ShownCropped(ImageRef);

    impl gpui_kit::Render for ShownCropped {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui_kit::Context<Self>,
        ) -> impl gpui_kit::IntoElement {
            use gpui_kit::{Styled as _, px};
            gpui_kit::img(cropped(&self.0, 100., 200.)).w(px(100.)).h(px(200.))
        }
    }
}
