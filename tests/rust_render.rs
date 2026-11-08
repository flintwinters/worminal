//! Private differential fixture driver; draws the product Rust painter, not a test reimplementation.
use gtk::prelude::*;
use std::{
    cell::RefCell,
    fs,
    io::Read,
    thread,
    time::{Duration, Instant},
};
use worminal_frontend::{
    paint::{Font, Painter, Palette, Style},
    viewport::Viewport,
};
use worminal_protocol::{Decoder, WORKSPACE_MAX_COLS, WORKSPACE_MAX_ROWS};

fn tabs(args: &[String]) {
    use std::rc::Rc;
    use worminal_frontend::{
        fonts::Fonts,
        tabs::{Tab, Tabs},
    };
    use worminal_protocol::Payload;
    assert_eq!(args.len(), 5);
    let bytes = fs::read(&args[1]).unwrap();
    let mut body = Payload::new(&bytes);
    let count = body.u32().unwrap();
    let active = body.u32().unwrap();
    let zoom = f64::from(body.u32().unwrap());
    let entries = (0..count)
        .map(|_| Tab {
            id: body.u32().unwrap(),
            browser: body.u32().unwrap() != 0,
            label: String::from_utf8(body.string().unwrap().to_vec()).unwrap(),
        })
        .collect();
    body.finish().unwrap();
    let width: i32 = args[3].parse().unwrap();
    let height: i32 = args[4].parse().unwrap();
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let tabs = Tabs::new(Rc::new(RefCell::new(Fonts::new(None, zoom))));
    tabs.update(
        entries,
        active,
        Some(Rc::new(RefCell::new(Palette::default()))),
    );
    window.add(tabs.widget());
    window.set_default_size(width, height);
    window.show_all();
    let settle = Instant::now() + Duration::from_millis(200);
    while Instant::now() < settle {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        thread::sleep(Duration::from_millis(1));
    }
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap();
    let cr = cairo::Context::new(&surface).unwrap();
    for _ in 0..21 {
        tabs.widget().draw(&cr);
    }
    surface
        .write_to_png(&mut fs::File::create(&args[2]).unwrap())
        .unwrap();
    window.close();
}

fn main() {
    gtk::init().expect("use manage.py check-rust-frontend on private displays");
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|value| value == "--tabs") {
        tabs(&args);
        return;
    }
    assert_eq!(args.len(), 13);
    let columns: u32 = args[2].parse().unwrap();
    let rows: u32 = args[3].parse().unwrap();
    let width: i32 = args[4].parse().unwrap();
    let height: i32 = args[5].parse().unwrap();
    assert!(columns <= WORKSPACE_MAX_COLS && rows <= WORKSPACE_MAX_ROWS);
    let window = gtk::Window::new(gtk::WindowType::Toplevel);
    let widget = gtk::DrawingArea::new();
    window.add(&widget);
    window.set_default_size(width, height);
    window.show_all();
    let settle = Instant::now() + Duration::from_millis(200);
    while Instant::now() < settle {
        while glib::MainContext::default().pending() {
            glib::MainContext::default().iteration(false);
        }
        thread::sleep(Duration::from_millis(1));
    }
    let layout = widget.create_pango_layout(None);
    let font = Font::resolve(&layout, None, 0.0);
    let painter = RefCell::new(Painter::new(layout, font));
    let mut viewport = Viewport::new(columns, rows).unwrap();
    let mut bytes = Vec::new();
    fs::File::open(&args[0])
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    let mut decoder = Decoder::default();
    let mut remaining = bytes.as_slice();
    let mut complete = 0;
    while !remaining.is_empty() {
        let (used, packet) = decoder.push(remaining).unwrap();
        assert!(used > 0);
        remaining = &remaining[used..];
        if let Some(packet) = packet {
            if viewport.packet(&packet).unwrap().is_some() {
                complete += 1;
            }
        }
    }
    assert_eq!(complete, 1);
    assert_eq!(viewport.frame().columns, columns as i32);
    assert_eq!(viewport.frame().rows, rows as i32);
    if args[10] == "1" {
        viewport.select(0, 0, true, 0, false, false).unwrap();
        viewport.select(5, 1, false, 0, false, false).unwrap();
    }
    let surface = cairo::ImageSurface::create(cairo::Format::ARgb32, width, height).unwrap();
    let cr = cairo::Context::new(&surface).unwrap();
    if args[11] == "1" {
        cr.translate(0.25, 0.375);
        cr.scale(1.25, 1.25);
    }
    if args[6] == "clipped" {
        cr.rectangle(0.0, f64::from(height / 2), f64::from(width), 16.0);
        cr.clip();
    }
    let mut palette = Palette::default();
    if args[12] == "1" {
        palette.set(2, Some("rgba(20,90,160,0.55)"));
    }
    let mode = args[7].parse().unwrap();
    let cursor = args[8].parse().unwrap();
    let focused = args[9] == "1";
    widget.connect_draw(move |_, cr| {
        painter
            .borrow_mut()
            .paint(
                &viewport,
                &Style {
                    palette: &palette,
                    mode,
                    cursor,
                    blink: false,
                    focused,
                    preedit: "",
                    link: None,
                },
                &cr,
            )
            .unwrap();
        glib::Propagation::Proceed
    });
    // Match the C probe's warm-up/paint count and GTK's fractional widget clipping.
    for _ in 0..21 {
        widget.draw(&cr);
    }
    surface
        .write_to_png(&mut fs::File::create(&args[1]).unwrap())
        .unwrap();
    window.close();
}
