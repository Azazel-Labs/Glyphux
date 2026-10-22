//! WebGPU demo: the shared `textra_gpu::demo::Scene` drawn into a `<canvas id="textra">` every
//! animation frame. Glyph fields are generated on the GPU on first use at each size level.
//! The page adds fonts and sets the text through the exported functions below.
//!
//! Build: see README.md (`wasm-bindgen` or `wasm-pack`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use textra_gpu::ShapingFont;
use textra_gpu::demo::Scene;
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;

static FONT: &[u8] = include_bytes!("../assets/Inter-Regular.otf");

/// Requests from the page, applied at the next frame (they may arrive before the scene exists).
enum Command {
    Text(String),
    Font {
        name: String,
        data: Arc<Vec<u8>>,
        index: u32,
    },
    Primary(String),
    WeightFont(Arc<Vec<u8>>),
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
/// collection (.ttc), `postscript_name` picks the face. Errors right away if the data is not a
/// font the scene can use (it is added at the next frame).
#[wasm_bindgen]
#[allow(clippy::needless_pass_by_value)] // wasm-bindgen passes owned values
pub fn add_font(name: String, data: Vec<u8>, postscript_name: Option<String>) -> Result<(), JsError> {
    let data = Arc::new(data);
    let index = face_index(&data, postscript_name.as_deref()).map_err(|e| JsError::new(&format!("{name}: {e}")))?;
    usable(&data, index).map_err(|e| JsError::new(&format!("{name}: {e}")))?;
    push(Command::Font { name, data, index });
    Ok(())
}

/// Whether the shaper (not only the outline loader) accepts face `index` of `data`.
fn usable(data: &Arc<Vec<u8>>, index: u32) -> Result<(), &'static str> {
    ShapingFont::new(0, data.clone(), index).map(drop).ok_or("not a font")
}

/// Sets the scene's text in the font added as `name` (others fill in missing characters).
#[wasm_bindgen]
pub fn set_primary_font(name: String) {
    push(Command::Primary(name));
}

/// A variable font with a `wght` axis for the title's breathing weight.
#[wasm_bindgen]
pub fn set_weight_font(data: Vec<u8>) -> Result<(), JsError> {
    let data = Arc::new(data);
    usable(&data, 0).map_err(|e| JsError::new(&format!("variable font: {e}")))?;
    push(Command::WeightFont(data));
    Ok(())
}

/// Shows or hides the atlas pages along the bottom.
#[wasm_bindgen]
pub fn set_show_atlas(show: bool) {
    push(Command::ShowAtlas(show));
}

/// The face of a font file or collection whose PostScript name is `postscript_name` (the
/// first face if no name is given).
fn face_index(data: &[u8], postscript_name: Option<&str>) -> Result<u32, String> {
    use textra_gpu::textra::io::font::ttf_parser::name_id;
    use textra_gpu::textra::io::font::{Font, FontScaling};
    // Each face is parsed once, until one fails (past the last face of a collection).
    let mut faces = (0..256).map_while(|i| {
        Font::from_bytes(data, i, FontScaling::EmNormalized)
            .ok()
            .map(|f| (i, f))
    });
    let Some(want) = postscript_name else {
        return faces.next().map(|(i, _)| i).ok_or_else(|| "not a font".to_owned());
    };
    faces
        .find(|(_, f)| {
            f.face()
                .names()
                .into_iter()
                .any(|n| n.name_id == name_id::POST_SCRIPT_NAME && n.to_string().as_deref() == Some(want))
        })
        .map(|(i, _)| i)
        .ok_or_else(|| format!("no face named {want}"))
}

/// Sets the text of the page element `id`.
fn show(id: &str, text: &str) {
    if let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id(id))
    {
        el.set_text_content(Some(text));
    }
}

/// Logs a problem and shows it in the page's note under the controls.
fn report(s: &str) {
    web_sys::console::error_1(&s.into());
    show("note", s);
}

#[wasm_bindgen(start)]
pub fn start() {
    // With panic = "abort" a panic surfaces as an opaque "unreachable" trap and the animation
    // loop stops: say what happened, in the console and on the page.
    std::panic::set_hook(Box::new(|info| {
        let msg = format!("textra demo stopped: {info}");
        web_sys::console::error_1(&msg.as_str().into());
        show("status", &msg);
    }));
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = run().await {
            web_sys::console::error_1(&format!("textra demo: {e}").into());
            show("status", &format!("Could not start: {e}"));
        }
    });
}

struct State {
    window: web_sys::Window,
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
        .get_element_by_id("textra")
        .ok_or("no <canvas id=textra>")?
        .dyn_into()
        .map_err(|_| "#textra is not a canvas")?;
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
            label: Some("textra demo"),
            // GPU timestamps for the on-screen timings, where the browser offers them.
            required_features: adapter.features() & wgpu::Features::TIMESTAMP_QUERY,
            required_limits: textra_gpu::required_limits().using_resolution(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::MemoryUsage,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(|e| format!("device: {e}"))?;
    // wgpu's default handler panics on validation errors; report them instead.
    device.on_uncaptured_error(Arc::new(|e| report(&format!("GPU error: {e}"))));
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
        window: window.clone(),
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
        let mut s = state.borrow_mut();
        frame(&mut s, now);
        if let Some(next) = f.borrow().as_ref()
            && s.window.request_animation_frame(next.as_ref().unchecked_ref()).is_err()
        {
            report("requestAnimationFrame failed: the animation stopped");
        }
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
                    report(&e);
                }
            }
            Command::Primary(name) => {
                if !s.scene.set_primary_font(&name) {
                    report(&format!("no font named {name}"));
                }
            }
            Command::WeightFont(data) => {
                if let Err(e) = s.scene.set_weight_font(data) {
                    report(&e);
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
    // A suboptimal texture is still drawn, then the surface is reconfigured for next time.
    let (tex, suboptimal) = match s.surface.get_current_texture() {
        wgpu::CurrentSurfaceTexture::Success(tex) => (tex, false),
        wgpu::CurrentSurfaceTexture::Suboptimal(tex) => (tex, true),
        wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
            s.surface.configure(&s.device, &s.config);
            return;
        }
        // Hidden, timed out or a reported error: skip the frame, try again on the next one.
        wgpu::CurrentSurfaceTexture::Timeout
        | wgpu::CurrentSurfaceTexture::Occluded
        | wgpu::CurrentSurfaceTexture::Validation => return,
    };
    let view = tex.texture.create_view(&wgpu::TextureViewDescriptor {
        format: Some(s.config.format.add_srgb_suffix()),
        ..Default::default()
    });
    // Seconds as f32 lose precision as they grow (steps of 2 ms after 9 hours, of a frame
    // after a few days): the clock restarts every 4.5 hours, a single jump in the animation.
    let t = (((now - s.start) / 1000.0) % 16384.0) as f32;
    // Lay out in CSS pixels: the same scene on any display. The scale comes from the backing
    // store itself, so it always agrees with the size being rendered.
    s.scene.set_scale(w as f32 / s.canvas.client_width().max(1) as f32);
    s.scene.render(&s.device, &s.queue, &view, [w, h], t);
    s.queue.present(tex);
    if suboptimal {
        s.surface.configure(&s.device, &s.config);
    }
}
