//! Owned shader-raster preparation inside the native asynchronous asset pipeline.
//! Native image requests and codecs retain their native loader/settings contract.

use crate::{Rgba8MipMode, TextureLayer, rgba8_mip_chain};
use bevy::asset::{AssetApp, AssetLoader, AssetPath, LoadContext, io::Reader};
use bevy::image::{
    CompressedImageFormatSupport, CompressedImageFormats, ImageLoader, ImageLoaderError,
    ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor,
};
use bevy::prelude::*;
use std::marker::PhantomData;
use wgpu_types::{TextureDimension, TextureFormat};

/// Typed filtering role. Distinct root types give each role an independent
/// native source identity even when the same file supplies several roles.
pub trait ImageMipRole: Send + Sync + TypePath + 'static {
    /// Texel filtering contract.
    const MODE: Rgba8MipMode;
    /// Native labeled image identity within its source.
    const LABEL: &'static str;
}
/// Linear-light color filtering of encoded sRGB texels.
#[derive(TypePath, Debug, Clone, PartialEq, Eq)]
pub struct ColorMip;
/// Scalar filtering of linear texels.
#[derive(TypePath, Debug, Clone, PartialEq, Eq)]
pub struct LinearMip;
/// Vector filtering and renormalization of tangent-space normals.
#[derive(TypePath, Debug, Clone, PartialEq, Eq)]
pub struct NormalMip;
impl ImageMipRole for ColorMip {
    const MODE: Rgba8MipMode = Rgba8MipMode::SrgbColor;
    const LABEL: &'static str = "mips/srgb";
}
impl ImageMipRole for LinearMip {
    const MODE: Rgba8MipMode = Rgba8MipMode::Linear;
    const LABEL: &'static str = "mips/linear";
}
impl ImageMipRole for NormalMip {
    const MODE: Rgba8MipMode = Rgba8MipMode::Normal;
    const LABEL: &'static str = "mips/normal";
}

/// Prepared source with its fully materialized native image dependency.
/// Keeping this handle on appearance intent retains native reload provenance.
#[derive(Asset, TypePath)]
pub struct PreparedShaderImage<M: ImageMipRole> {
    /// GPU-compatible child published together with the prepared source.
    #[dependency]
    pub image: Handle<Image>,
    role: PhantomData<M>,
}

/// A texture is either an owner-supplied native image or a typed raster request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ShaderTexture {
    /// Native image supplied by an importer, generator or dedicated binding.
    Image(Handle<Image>),
    /// Encoded color source prepared on the asset worker.
    Color(Handle<PreparedShaderImage<ColorMip>>),
    /// Linear scalar source prepared on the asset worker.
    Linear(Handle<PreparedShaderImage<LinearMip>>),
    /// Normal-vector source prepared on the asset worker.
    Normal(Handle<PreparedShaderImage<NormalMip>>),
}
impl From<Handle<Image>> for ShaderTexture {
    fn from(image: Handle<Image>) -> Self {
        Self::Image(image)
    }
}

/// A raster request must address a physical source, independent of labels
/// assigned by a container importer. Imported images use that owner's handle.
#[derive(Debug, thiserror::Error)]
pub enum RasterSourceError {
    /// Native labeled loading selects the container's loader, not a raster root.
    #[error("raster source `{0}` has a container label; bind its importer-owned image handle")]
    ContainerLabel(AssetPath<'static>),
}

impl ShaderTexture {
    /// Stable native asset identity of this request or supplied image.
    pub fn id(&self) -> bevy::asset::UntypedAssetId {
        match self {
            Self::Image(h) => h.id().untyped(),
            Self::Color(h) => h.id().untyped(),
            Self::Linear(h) => h.id().untyped(),
            Self::Normal(h) => h.id().untyped(),
        }
    }
    /// Native image reference, when supplied by its owner.
    pub fn image(&self) -> Option<&Handle<Image>> {
        if let Self::Image(image) = self {
            Some(image)
        } else {
            None
        }
    }
    /// Request a physical raster source with explicit role and sampler quality.
    /// Native labeled container images bind through their importer-owned handle.
    pub fn load_raster(
        server: &AssetServer,
        path: AssetPath<'static>,
        layer: TextureLayer,
        anisotropy: u16,
    ) -> Result<Self, RasterSourceError> {
        if path.label().is_some() {
            return Err(RasterSourceError::ContainerLabel(path));
        }
        let mode = match layer {
            TextureLayer::Albedo | TextureLayer::Mineral | TextureLayer::ContinuationAlbedo => {
                Some(Rgba8MipMode::SrgbColor)
            }
            TextureLayer::Surface | TextureLayer::ContinuationSurface => Some(Rgba8MipMode::Linear),
            TextureLayer::Normal => Some(Rgba8MipMode::Normal),
            TextureLayer::Height | TextureLayer::ShadowCache | TextureLayer::SurfaceAnnotations => {
                None
            }
        };
        let load =
            server
                .load_builder()
                .with_settings(move |settings: &mut ImageLoaderSettings| {
                    settings.is_srgb = mode == Some(Rgba8MipMode::SrgbColor);
                    if mode.is_some() {
                        let mut sampler = ImageSamplerDescriptor::linear();
                        sampler.set_anisotropic_filter(anisotropy);
                        settings.sampler = ImageSampler::Descriptor(sampler);
                    }
                });
        Ok(match mode {
            Some(Rgba8MipMode::SrgbColor) => Self::Color(load.load(path)),
            Some(Rgba8MipMode::Linear) => Self::Linear(load.load(path)),
            Some(Rgba8MipMode::Normal) => Self::Normal(load.load(path)),
            None => Self::Image(load.load(path)),
        })
    }
}

/// Install after the host's native image/render plugins. Only typed prepared
/// roots select these loaders; no file extension or native Image loader is replaced.
pub struct LuncoImagePlugin;
impl Plugin for LuncoImagePlugin {
    fn build(&self, app: &mut App) {
        app.init_asset::<PreparedShaderImage<ColorMip>>()
            .init_asset::<PreparedShaderImage<LinearMip>>()
            .init_asset::<PreparedShaderImage<NormalMip>>();
    }
    fn finish(&self, app: &mut App) {
        let formats = app
            .world()
            .get_resource::<CompressedImageFormatSupport>()
            .map_or(CompressedImageFormats::NONE, |support| support.0);
        app.register_asset_loader(PreparedImageLoader::<ColorMip>::new(formats))
            .register_asset_loader(PreparedImageLoader::<LinearMip>::new(formats))
            .register_asset_loader(PreparedImageLoader::<NormalMip>::new(formats));
    }
}
#[derive(TypePath)]
struct PreparedImageLoader<M: ImageMipRole> {
    decoder: ImageLoader,
    role: PhantomData<M>,
}
impl<M: ImageMipRole> PreparedImageLoader<M> {
    fn new(formats: CompressedImageFormats) -> Self {
        Self {
            decoder: ImageLoader::new(formats),
            role: PhantomData,
        }
    }
}
#[derive(Debug, thiserror::Error)]
enum ImagePreparationError {
    #[error(transparent)]
    Decode(#[from] ImageLoaderError),
    #[error("invalid RGBA8 base pixels for role-aware mip preparation")]
    InvalidBase,
}
impl<M: ImageMipRole> AssetLoader for PreparedImageLoader<M> {
    type Asset = PreparedShaderImage<M>;
    type Settings = ImageLoaderSettings;
    type Error = ImagePreparationError;
    async fn load(
        &self,
        reader: &mut dyn Reader,
        settings: &ImageLoaderSettings,
        context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let image = self.decoder.load(reader, settings, context).await?;
        // Await owned CPU work without occupying an I/O worker or borrowing ECS.
        let image = bevy::tasks::AsyncComputeTaskPool::get()
            .spawn(async move {
                let _span =
                    bevy::log::info_span!("lunco_shader_raster_prepare", mode = ?M::MODE).entered();
                let mut image = image;
                prepare_image(&mut image, M::MODE)?;
                Ok::<_, ImagePreparationError>(image)
            })
            .await?;
        // Shader consumers read handles/descriptors, never resident texels.
        // Native GpuImage extraction moves data and retains source metadata.
        let mut image = image;
        image.asset_usage = bevy::asset::RenderAssetUsages::RENDER_WORLD;
        let image = context.add_labeled_asset(M::LABEL.to_string(), image);
        Ok(PreparedShaderImage {
            image,
            role: PhantomData,
        })
    }
}
fn prepare_image(image: &mut Image, mode: Rgba8MipMode) -> Result<(), ImagePreparationError> {
    let descriptor = &image.texture_descriptor;
    // Native compressed chains, arrays and other formats retain their layout.
    if descriptor.mip_level_count > 1
        || descriptor.dimension != TextureDimension::D2
        || descriptor.size.depth_or_array_layers != 1
        || !matches!(
            descriptor.format,
            TextureFormat::Rgba8Unorm | TextureFormat::Rgba8UnormSrgb
        )
    {
        return Ok(());
    }
    let expected = match mode {
        Rgba8MipMode::SrgbColor => TextureFormat::Rgba8UnormSrgb,
        Rgba8MipMode::Linear | Rgba8MipMode::Normal => TextureFormat::Rgba8Unorm,
    };
    if descriptor.format != expected {
        return Err(ImagePreparationError::InvalidBase);
    }
    let width = descriptor.size.width as usize;
    let height = descriptor.size.height as usize;
    let base = image
        .data
        .take()
        .ok_or(ImagePreparationError::InvalidBase)?;
    let (data, levels) =
        rgba8_mip_chain(base, width, height, mode).ok_or(ImagePreparationError::InvalidBase)?;
    image.data = Some(data);
    image.texture_descriptor.mip_level_count = levels;
    Ok(())
}

/// Read-only prepared-source stores shared by rendering consumers.
#[derive(bevy::ecs::system::SystemParam)]
pub struct ShaderImageAssets<'w> {
    colors: Res<'w, Assets<PreparedShaderImage<ColorMip>>>,
    linear: Res<'w, Assets<PreparedShaderImage<LinearMip>>>,
    normals: Res<'w, Assets<PreparedShaderImage<NormalMip>>>,
}
impl ShaderImageAssets<'_> {
    /// Resolve a native image only after its complete source is published.
    pub fn get<'a>(&'a self, source: &'a ShaderTexture) -> Option<&'a Handle<Image>> {
        match source {
            ShaderTexture::Image(h) => Some(h),
            ShaderTexture::Color(h) => self.colors.get(h).map(|a| &a.image),
            ShaderTexture::Linear(h) => self.linear.get(h).map(|a| &a.image),
            ShaderTexture::Normal(h) => self.normals.get(h).map(|a| &a.image),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::asset::{
        AssetPlugin,
        io::{
            AssetSourceBuilder, AssetSourceId,
            memory::{Dir, MemoryAssetReader},
        },
    };
    use bevy::image::ImageFilterMode;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    // Inline PNG codec fixtures, independent of repository and Twin assets.
    const DARK: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 80, 80, 80, 248, 15,
        194, 12, 48, 6, 0, 44, 52, 5, 125, 72, 187, 215, 130, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66,
        96, 130,
    ];
    const LIGHT: &[u8] = &[
        137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 2, 0, 0, 0, 2, 8, 6,
        0, 0, 0, 114, 182, 13, 36, 0, 0, 0, 17, 73, 68, 65, 84, 120, 156, 99, 120, 240, 224, 193,
        127, 16, 102, 128, 49, 0, 129, 180, 14, 125, 139, 2, 216, 129, 0, 0, 0, 0, 73, 69, 78, 68,
        174, 66, 96, 130,
    ];
    fn update_until(app: &mut App, mut ready: impl FnMut(&World) -> bool) {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            app.update();
            if ready(app.world()) {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        panic!("image asset did not reach its expected terminal state");
    }
    #[test]
    fn image_load_and_reload_publish_complete_chains_without_replacing_native_loading() {
        let dir = Dir::default();
        dir.insert_asset(Path::new("raster.png"), DARK);
        dir.insert_asset(Path::new("invalid.png"), b"invalid PNG".as_slice());
        let source = dir.clone();
        let mut app = App::new();
        app.register_asset_source(
            AssetSourceId::Default,
            AssetSourceBuilder::new(move || {
                Box::new(MemoryAssetReader {
                    root: source.clone(),
                })
            }),
        );
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::image::ImagePlugin::default(),
        ));
        app.register_asset_loader(ImageLoader::new(CompressedImageFormats::NONE));
        app.add_plugins(LuncoImagePlugin);
        app.finish();
        app.cleanup();
        let server = app.world().resource::<AssetServer>().clone();
        let native = server
            .load_builder()
            .with_settings(|s: &mut ImageLoaderSettings| s.is_srgb = false)
            .load::<Image>("raster.png");
        let color =
            ShaderTexture::load_raster(&server, "raster.png".into(), TextureLayer::Albedo, 8)
                .unwrap();
        let scalar =
            ShaderTexture::load_raster(&server, "raster.png".into(), TextureLayer::Surface, 8)
                .unwrap();
        let normal =
            ShaderTexture::load_raster(&server, "raster.png".into(), TextureLayer::Normal, 8)
                .unwrap();
        assert_ne!(color.id(), scalar.id());
        assert_ne!(normal.id(), scalar.id());
        let invalid =
            ShaderTexture::load_raster(&server, "invalid.png".into(), TextureLayer::Albedo, 8)
                .unwrap();
        let inspect = |world: &World, value: u8| -> bool {
            let mut all = true;
            let images = world.resource::<Assets<Image>>();
            for (source, mode) in [
                (&color, Rgba8MipMode::SrgbColor),
                (&scalar, Rgba8MipMode::Linear),
                (&normal, Rgba8MipMode::Normal),
            ] {
                let child = match source {
                    ShaderTexture::Color(h) => world
                        .resource::<Assets<PreparedShaderImage<ColorMip>>>()
                        .get(h)
                        .map(|a| &a.image),
                    ShaderTexture::Linear(h) => world
                        .resource::<Assets<PreparedShaderImage<LinearMip>>>()
                        .get(h)
                        .map(|a| &a.image),
                    ShaderTexture::Normal(h) => world
                        .resource::<Assets<PreparedShaderImage<NormalMip>>>()
                        .get(h)
                        .map(|a| &a.image),
                    _ => unreachable!(),
                };
                if let Some(child) = child {
                    let image = images
                        .get(child)
                        .expect("child published before source readiness");
                    assert_eq!(
                        image.asset_usage,
                        bevy::asset::RenderAssetUsages::RENDER_WORLD
                    );
                    assert_eq!(image.texture_descriptor.mip_level_count, 2);
                    let pixels = image.data.as_ref().expect("pixels");
                    let source_value = pixels[0];
                    let expected = rgba8_mip_chain(
                        [source_value, source_value, source_value, 255].repeat(4),
                        2,
                        2,
                        mode,
                    )
                    .unwrap()
                    .0;
                    assert_eq!(pixels, &expected);
                    let ImageSampler::Descriptor(sampler) = &image.sampler else {
                        panic!("missing sampler")
                    };
                    assert_eq!(sampler.mipmap_filter, ImageFilterMode::Linear);
                    assert_eq!(sampler.anisotropy_clamp, 8);
                    all &= source_value == value;
                } else {
                    all = false;
                }
            }
            all
        };
        update_until(&mut app, |world| {
            inspect(world, 32)
                && images_native_ready(world, &native)
                && server
                    .get_load_state(invalid.id())
                    .is_some_and(|s| s.is_failed())
        });
        let native_image = app
            .world()
            .resource::<Assets<Image>>()
            .get(&native)
            .unwrap();
        assert_eq!(
            native_image.texture_descriptor.format,
            TextureFormat::Rgba8Unorm
        );
        assert_eq!(native_image.texture_descriptor.mip_level_count, 1);
        assert!(matches!(native_image.sampler, ImageSampler::Default));
        dir.insert_asset(Path::new("raster.png"), LIGHT);
        server.reload("raster.png");
        update_until(&mut app, |world| inspect(world, 224));
    }
    fn images_native_ready(world: &World, handle: &Handle<Image>) -> bool {
        world.resource::<Assets<Image>>().get(handle).is_some()
    }

    #[derive(Asset, TypePath)]
    struct ImageContainer;

    #[derive(TypePath)]
    struct ImageContainerLoader;

    impl AssetLoader for ImageContainerLoader {
        type Asset = ImageContainer;
        type Settings = ();
        type Error = std::io::Error;

        async fn load(
            &self,
            reader: &mut dyn Reader,
            _: &(),
            context: &mut LoadContext<'_>,
        ) -> Result<ImageContainer, Self::Error> {
            let mut source = Vec::new();
            reader.read_to_end(&mut source).await?;
            let value = *source.first().ok_or_else(|| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "empty image container")
            })?;
            let mut image = Image::default();
            image.texture_descriptor.size.width = 2;
            image.texture_descriptor.size.height = 2;
            image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
            let (data, levels) = rgba8_mip_chain(
                [value, value, value, 255].repeat(4),
                2,
                2,
                Rgba8MipMode::SrgbColor,
            )
            .unwrap();
            image.data = Some(data);
            image.texture_descriptor.mip_level_count = levels;
            context.add_labeled_asset("texture".to_string(), image);
            Ok(ImageContainer)
        }

        fn extensions(&self) -> &[&str] {
            &["image-container"]
        }
    }

    #[test]
    fn image_native_container_labels_reload_and_raster_requests_reject_labels() {
        let dir = Dir::default();
        dir.insert_asset(Path::new("source.image-container"), &[32][..]);
        let source = dir.clone();
        let mut app = App::new();
        app.register_asset_source(
            AssetSourceId::Default,
            AssetSourceBuilder::new(move || {
                Box::new(MemoryAssetReader {
                    root: source.clone(),
                })
            }),
        );
        app.add_plugins((
            MinimalPlugins,
            AssetPlugin::default(),
            bevy::image::ImagePlugin::default(),
        ));
        app.init_asset::<ImageContainer>()
            .register_asset_loader(ImageContainerLoader)
            .add_plugins(LuncoImagePlugin);
        app.finish();
        app.cleanup();
        let server = app.world().resource::<AssetServer>().clone();
        let path: AssetPath = "source.image-container#texture".into();
        let error =
            ShaderTexture::load_raster(&server, path.clone(), TextureLayer::Albedo, 8).unwrap_err();
        assert!(
            matches!(error, RasterSourceError::ContainerLabel(ref rejected) if rejected == &path)
        );
        assert!(
            server
                .get_handle::<PreparedShaderImage<ColorMip>>(path.clone())
                .is_none()
        );
        let image = server.load::<Image>(path);
        let look = crate::ShaderLook::new("native-shader.wgsl")
            .with_texture(TextureLayer::Albedo, image.clone());
        assert_eq!(look.textures[&TextureLayer::Albedo].image(), Some(&image));
        let ready = |world: &World, expected| {
            world
                .resource::<Assets<Image>>()
                .get(&image)
                .is_some_and(|image| {
                    assert_eq!(image.texture_descriptor.mip_level_count, 2);
                    image.data.as_ref().is_some_and(|data| data[0] == expected)
                })
        };
        update_until(&mut app, |world| ready(world, 32));
        dir.insert_asset(Path::new("source.image-container"), &[224][..]);
        server.reload("source.image-container");
        update_until(&mut app, |world| ready(world, 224));
    }

    #[test]
    fn image_preparation_rejects_invalid_pixels_and_preserves_native_chains() {
        let mut image = Image::default();
        image.texture_descriptor.format = TextureFormat::Rgba8UnormSrgb;
        image.data = None;
        assert!(prepare_image(&mut image, Rgba8MipMode::SrgbColor).is_err());
        image.data = Some(vec![0]);
        assert!(prepare_image(&mut image, Rgba8MipMode::SrgbColor).is_err());
        image.data = Some(vec![0; 4]);
        assert!(prepare_image(&mut image, Rgba8MipMode::Normal).is_err());
        image.texture_descriptor.mip_level_count = 2;
        let expected = image.clone();
        prepare_image(&mut image, Rgba8MipMode::SrgbColor).unwrap();
        assert_eq!(image, expected);
    }
}
