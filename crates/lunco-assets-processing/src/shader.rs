//! Optional offline WESL image kernels. GPU f32 conversion is explicit here;
//! canonical DEM preparation stays in the native f64 raster processors.

use crate::process::ProcessControl;
use lunco_assets_datasets::ProcessConfig;
use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};
use wgpu::util::DeviceExt;

fn invalid(message: impl Into<String>) -> std::io::Error {
    std::io::Error::new(std::io::ErrorKind::InvalidInput, message.into())
}

const IMAGE_ABI: &str = include_str!("image_kernel.wgsl");

pub(crate) fn process(
    input: &Path,
    output: &Path,
    config: &ProcessConfig,
    values: &BTreeMap<String, PathBuf>,
    control: &ProcessControl,
) -> std::io::Result<()> {
    let code_id = config
        .parameters
        .get("shader")
        .and_then(toml::Value::as_str)
        .ok_or_else(|| invalid("shader step requires a declared shader code input"))?;
    if !values.contains_key(code_id) {
        return Err(invalid("shader code is not a declared input"));
    }
    let mut resolver = wesl::resolver::VirtualResolver::new();
    for (id, path) in values {
        if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| matches!(e, "wesl" | "wgsl"))
        {
            let mut text = std::fs::read_to_string(path)?;
            if id == code_id {
                text.push_str(IMAGE_ABI);
            }
            resolver.add_module(
                id.parse()
                    .map_err(|e| invalid(format!("shader module: {e}")))?,
                Cow::Owned(text),
            );
        }
    }
    let module = code_id
        .parse()
        .map_err(|e| invalid(format!("shader module: {e}")))?;
    let compiled = wesl::compile(&module, &wesl::CompileOptions::default(), &resolver)
        .map_err(|e| invalid(e.to_string()))?;
    let input_space = color_space(config, "input_color_space")?;
    let output_space = color_space(config, "output_color_space")?;
    let pixels = image::open(input)
        .map_err(|e| invalid(e.to_string()))?
        .to_rgba32f();
    let (iw, ih) = pixels.dimensions();
    let [ow, oh] = config
        .target_resolution
        .ok_or_else(|| invalid("shader step requires target_resolution"))?;
    if ow == 0 || oh == 0 {
        return Err(invalid("shader dimensions must be positive"));
    }
    let mut input_bytes = Vec::with_capacity(pixels.as_raw().len() * 4);
    for pixel in pixels.pixels() {
        for (channel, value) in pixel.0.iter().enumerate() {
            let value = if input_space == "srgb" && channel < 3 {
                decode_srgb(*value)
            } else {
                *value
            };
            input_bytes.extend(value.to_ne_bytes());
        }
    }
    let params = config
        .parameters
        .get("uniforms")
        .and_then(toml::Value::as_array)
        .filter(|v| !v.is_empty())
        .ok_or_else(|| invalid("shader uniforms must be a non-empty numeric array"))?;
    let mut param_bytes = Vec::new();
    for value in params {
        let value = value
            .as_float()
            .or_else(|| value.as_integer().map(|v| v as f64))
            .ok_or_else(|| invalid("shader uniforms must be numeric"))?;
        if !value.is_finite() || value.abs() > f32::MAX as f64 {
            return Err(invalid("shader uniform cannot cross the GPU f32 boundary"));
        }
        param_bytes.extend((value as f32).to_ne_bytes());
    }
    let timeout = config
        .parameters
        .get("timeout_s")
        .and_then(toml::Value::as_integer)
        .filter(|v| *v > 0)
        .ok_or_else(|| invalid("shader step requires a positive timeout_s"))?;
    let result = futures_lite::future::block_on(run(
        &compiled.to_string(),
        &input_bytes,
        &param_bytes,
        [iw, ih, ow, oh],
        timeout as u64,
        control,
    ))?;
    let mut rgba = Vec::with_capacity((ow as usize) * (oh as usize) * 4);
    for (index, bytes) in result.chunks_exact(4).enumerate() {
        let value = f32::from_ne_bytes(
            bytes
                .try_into()
                .map_err(|_| invalid("GPU channel is incomplete"))?,
        );
        if !value.is_finite() {
            return Err(invalid("shader produced non-finite image data"));
        }
        let value = if output_space == "srgb" && index % 4 < 3 {
            encode_srgb(value.clamp(0.0, 1.0))
        } else {
            value
        };
        rgba.push((value.clamp(0.0, 1.0) * 255.0).round() as u8);
    }
    image::RgbaImage::from_raw(ow, oh, rgba)
        .ok_or_else(|| invalid("GPU result has incorrect image dimensions"))?
        .save(output)
        .map_err(|e| invalid(e.to_string()))
}

fn color_space<'a>(config: &'a ProcessConfig, field: &str) -> std::io::Result<&'a str> {
    match config.parameters.get(field).and_then(toml::Value::as_str) {
        Some(v @ ("linear" | "srgb")) => Ok(v),
        _ => Err(invalid(format!(
            "shader {field} must be explicitly linear or srgb"
        ))),
    }
}
fn decode_srgb(v: f32) -> f32 {
    if v <= 0.04045 {
        v / 12.92
    } else {
        ((v + 0.055) / 1.055).powf(2.4)
    }
}
fn encode_srgb(v: f32) -> f32 {
    if v <= 0.0031308 {
        v * 12.92
    } else {
        1.055 * v.powf(1.0 / 2.4) - 0.055
    }
}

async fn run(
    source: &str,
    input: &[u8],
    params: &[u8],
    size: [u32; 4],
    timeout: u64,
    control: &ProcessControl,
) -> std::io::Result<Vec<u8>> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions::default())
        .await
        .map_err(|e| invalid(e.to_string()))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default())
        .await
        .map_err(|e| invalid(e.to_string()))?;
    let output_size = size[2] as u64 * size[3] as u64 * 16;
    let limit = device.limits().max_storage_buffer_binding_size as u64;
    if input.len() as u64 > limit || output_size > limit || params.len() as u64 > limit {
        return Err(invalid(
            "shader image exceeds this adapter's storage-buffer limit",
        ));
    }
    let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("artifact image kernel"),
        source: wgpu::ShaderSource::Wgsl(source.into()),
    });
    let layout_entries: Vec<_> = (0..4)
        .map(|binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: if binding == 2 {
                    wgpu::BufferBindingType::Uniform
                } else {
                    wgpu::BufferBindingType::Storage {
                        read_only: binding != 1,
                    }
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        })
        .collect();
    let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &layout_entries,
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bindings)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: None,
        layout: Some(&layout),
        module: &module,
        entry_point: Some("artifact_image_main"),
        compilation_options: Default::default(),
        cache: None,
    });
    if let Some(error) = errors.pop().await {
        return Err(invalid(error.to_string()));
    }
    let errors = device.push_error_scope(wgpu::ErrorFilter::Validation);
    let input = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: input,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let params = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: params,
        usage: wgpu::BufferUsages::STORAGE,
    });
    let size: Vec<u8> = size.into_iter().flat_map(u32::to_ne_bytes).collect();
    let dimensions = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: &size,
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let output = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: output_size,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let readback = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: output_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let buffers = [&input, &output, &dimensions, &params];
    let entries: Vec<_> = buffers
        .into_iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipeline.get_bind_group_layout(0),
        entries: &entries,
    });
    let mut encoder = device.create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_compute_pass(&Default::default());
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &group, &[]);
        let width = u32::from_ne_bytes(
            size[8..12]
                .try_into()
                .map_err(|_| invalid("missing output width"))?,
        );
        let height = u32::from_ne_bytes(
            size[12..16]
                .try_into()
                .map_err(|_| invalid("missing output height"))?,
        );
        pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
    }
    encoder.copy_buffer_to_buffer(&output, 0, &readback, 0, output_size);
    queue.submit([encoder.finish()]);
    if let Some(error) = errors.pop().await {
        return Err(invalid(error.to_string()));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    readback
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            let _ = tx.send(result);
        });
    let start = Instant::now();
    loop {
        control.check()?;
        if start.elapsed() > Duration::from_secs(timeout) {
            return Err(invalid("shader image readback timed out"));
        }
        device
            .poll(wgpu::PollType::Poll)
            .map_err(|e| invalid(e.to_string()))?;
        match rx.try_recv() {
            Ok(result) => {
                result.map_err(|e| invalid(e.to_string()))?;
                break;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {
                std::thread::sleep(Duration::from_millis(5))
            }
            Err(error) => return Err(invalid(error.to_string())),
        }
    }
    let bytes = readback
        .slice(..)
        .get_mapped_range()
        .map_err(|e| invalid(e.to_string()))?
        .to_vec();
    readback.unmap();
    Ok(bytes)
}
