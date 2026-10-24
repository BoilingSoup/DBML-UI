use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use tui_db::app::SAMPLE_DBML;
use tui_db::dbml::parser;
use tui_db::layout::layout;
use tui_db::model::build;
use tui_db::render::text;

fn main() {
    let out = parser::parse(SAMPLE_DBML);
    let schema = build(&out.doc);
    let l = layout(&schema);
    let area = Rect::new(0, 0, 110, 40);
    let mut buf = Buffer::empty(area);
    text::render(&mut buf, area, &l, (0, 0));
    for y in 0..area.height {
        let mut line = String::new();
        for x in 0..area.width {
            line.push_str(
                buf.cell(ratatui::layout::Position::new(x, y))
                    .unwrap()
                    .symbol(),
            );
        }
        println!("{y:2}|{}", line.trim_end());
    }
    println!(
        "boxes={} edges={} dangling={} errors={}",
        l.boxes.len(),
        l.edges.len(),
        schema.dangling,
        out.errors.len()
    );
}
