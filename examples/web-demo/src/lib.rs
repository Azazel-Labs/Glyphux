//! WebGPU demo: the shared `msdfr_gpu::demo::Scene` drawn into a `<canvas id="msdfr">` every
//! animation frame. Glyph fields are generated on the GPU on first use at each size level.
//! The page adds fonts and sets the text through the exported functions below.
//!
//! Build: see README.md (`wasm-bindgen` or `wasm-pack`).

use std::cell::RefCell;
use std::rc::Rc;

use msdfr_gpu::demo::Scene;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

static FONT: &[u8] = include_bytes!("../assets/Inter-Regular.otf");

/// Requests from the page, applied at the next frame (they may arrive before the scene exists).
enum Command {
    Text(String),
    Font { name: String, data: Vec<u8>, index: u32 },
    Primary(String),
    WeightFont(Vec<u8>),
    ShowAtlas(bool),
}

thread_local! {
    static COMMANDS: RefCell<Vec<Command>> = const { RefCell::new(Vec::new()) };
}

fn push(c: Command) {
    COMMANDS.with(|q| q.borrow_mut().push(c));
}

/// The text flowing along the spline.
#[wasm_bindgen]
pub fn set_text(text: String) {
    push(Command::Text(text));
}

/// Adds a font to the fallback list under `name` (replacing one of the same name). For a font
/// collection (.ttc), `postscript_name` picks the face. Errors if the data is not a font.
#[wasm_bindgen]
#[allow(clippy::needless_pass_by_value)] // wasm-bindgen passes owned values
pub fn add_font(name: String, data: Vec<u8>, postscript_name: Option<String>) -> Result<(), JsError> {
    let index =
        face_index(&data, postscript_name.as_deref()).ok_or_else(|| JsError::new(&format!("{name}: not a font")))?;
    push(Command::Font { name, data, index });
    Ok(())
}

/// Sets the scene's text in the font added as `name` (others fill in missing characters).
#[wasm_bindgen]
pub fn set_primary_font(name: String) {
    push(Command::Primary(name));
}

/// A variable font with a `wght` axis for the title's breathing weight.
#[wasm_bindgen]
pub fn set_weight_font(data: Vec<u8>) {
    push(Command::WeightFont(data));
}

/// Shows or hides the atlas pages along the bottom.
#[wasm_bindgen]
pub fn set_show_atlas(show: bool) {
    push(Command::ShowAtlas(show));
}

/// The face of a font file or collection whose PostScript name is `postscript_name` (the
/// first face if there is none or no name is given); `None` if `data` is not a font.
fn face_index(data: &[u8], postscript_name: Option<&str>) -> Option<u32> {
    use msdfr_gpu::msdfr::io::font::ttf_parser::name_id;
    use msdfr_gpu::msdfr::io::font::{Font, FontScaling};
    Font::from_bytes(data, 0, FontScaling::EmNormalized).ok()?;
    let Some(want) = postscript_name else {
        return Some(0);
    };
    let matches = |i: u32| {
        Font::from_bytes(data, i, FontScaling::EmNormalized).is_ok_and(|f| {
            f.face()
                .names()
                .into_iter()
                .any(|n| n.name_id == name_id::POST_SCRIPT_NAME && n.to_string().as_deref() == Some(want))
        })
    };
    Some(
        (0..256)
            .take_while(|&i| Font::from_bytes(data, i, FontScaling::EmNormalized).is_ok())
            .find(|&i| matches(i))
            .unwrap_or(0),
    )
}

fn log(s: &str) {
    web_sys::console::log_1(&s.into());
}

#[wasm_bindgen(start)]
pub fn start() {
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = run().await {
            log(&format!("msdfr demo: {e}"));
            if let Some(el) = web_sys::window()
                .and_then(|w| w.document())
                .and_then(|d| d.get_element_by_id("status"))
            {
                el.set_text_content(Some(&format!("Could not start: {e}")));
            }
        }
    });
}

struct State {
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    canvas: web_sys::HtmlCanvasElement,
    scene: Scene,
    start: f64,
}

async fn run() -> Result<(), String> {
    let window = web_sys::window().ok_or("no window")?;
    let document = window.document().ok_or("no document")?;
    let canvas: web_sys::HtmlCanvasElement = document
        .get_element_by_id("msdfr")
        .ok_or("no <canvas id=msdfr>")?
        .dyn_into()
        .map_err(|_| "#msdfr is not a canvas")?;
    let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
    desc.backends = wgpu::Backends::BROWSER_WEBGPU;
    let instance = wgpu::Instance::new(desc);
    let surface = instance
        .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
        .map_err(|e| format!("surface: {e}"))?;
    let adapter = instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("WebGPU adapter: {e}"))?;
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: Some("msdfr demo"),
            // GPU timestamps for the on-screen timings, where the browser offers them.
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            required_limits: msdfr_gpu::required_limits().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|e| format!("device: {e}"))?;
    let caps = surface.get_capabilities(&adapter);
    // Prefer an sRGB view of the swapchain (colors are blended in linear space).
    let format = caps.formats[0];
    let view_format = format.add_srgb_suffix();
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format,
        color_space: wgpu::SurfaceColorSpace::Auto,
        width: canvas.width().max(1),
        height: canvas.height().max(1),
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: caps.alpha_modes[0],
        view_formats: if view_format == format {
            vec![]
        } else {
            vec![view_format]
        },
    };
    surface.configure(&device, &config);
    let mut scene = Scene::new(&device, view_format, "Inter", FONT.to_vec())?;
    scene.enable_profiling(&device, &queue);
    let start = window.performance().map_or(0.0, |p| p.now());
    let state = Rc::new(RefCell::new(State {
        device,
        queue,
        surface,
        config,
        canvas,
        scene,
        start,
    }));

    // requestAnimationFrame loop.
    type FrameCallback = Rc<RefCell<Option<Closure<dyn FnMut(f64)>>>>;
    let f: FrameCallback = Rc::new(RefCell::new(None));
    let g = f.clone();
    *g.borrow_mut() = Some(Closure::new(move |now: f64| {
        frame(&mut state.borrow_mut(), now);
        let w = web_sys::window().unwrap();
        let _ = w.request_animation_frame(f.borrow().as_ref().unwrap().as_ref().unchecked_ref());
    }));
    window
        .request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref())
        .map_err(|_| "requestAnimationFrame")?;
    if let Some(el) = document.get_element_by_id("status") {
        el.set_text_content(Some(""));
    }
    Ok(())
}

fn frame(s: &mut State, now: f64) {
    for c in COMMANDS.with(|q| std::mem::take(&mut *q.borrow_mut())) {
        match c {
            Command::Text(t) => s.scene.set_text(&t),
            Command::Font { name, data, index } => {
                if let Err(e) = s.scene.add_font(&name, data, index) {
                    log(&e);
                }
            }
            Command::Primary(name) => {
                if !s.scene.set_primary_font(&name) {
                    log(&format!("no font named {name}"));
                }
            }
            Command::WeightFont(data) => {
                if let Err(e) = s.scene.set_weight_font(data) {
                    log(&e);
                }
            }
            Command::ShowAtlas(show) => s.scene.show_atlas = show,
        }
    }
    // Follow the canvas size (index.html keeps it at CSS size x devicePixelRatio).
    let (w, h) = (s.canvas.width().max(1), s.canvas.height().max(1));
    if (w, h) != (s.config.width, s.config.height) {
        s.config.width = w;
        s.config.height = h;
        s.surface.configure(&s.device, &s.config);
    }
    let (wgpu::CurrentSurfaceTexture::Success(tex) | wgpu::CurrentSurfaceTexture::Suboptimal(tex)) =
        s.surface.get_current_texture()
    else {
        s.surface.configure(&s.device, &s.config);
        return;
    };
    let view = tex.texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(s.config.format.add_srgb_suffix()),
        ..Default::default()
    });
    let t = ((now - s.start) / 1000.0) as f32;
    // Lay out in CSS pixels: the same scene on any display.
    if let Some(w) = web_sys::window() {
        s.scene.set_scale(w.device_pixel_ratio() as f32);
    }
    s.scene.render(&s.device, &s.queue, &view, [w, h], t);
    s.queue.present(tex);
}
