use crate::{
    Result,
    assets::{Asset, Assets, Fonts},
    markdown::{Block, Inline, Style},
    theme::{Color, Theme},
};
use comrak::nodes::TableAlignment;
use krilla::{
    Document,
    action::LinkAction,
    annotation::{Annotation, LinkAnnotation, Target},
    geom::{PathBuilder, Point, Rect, Size, Transform},
    metadata::Metadata,
    page::PageSettings,
    paint::Fill,
    text::TextDirection,
};
use krilla_svg::{SurfaceExt, SvgSettings};
use std::path::Path;

const LEFT: f32 = 54.0;
const TOP: f32 = 57.0;
const BOTTOM: f32 = 63.0;
const SIZE: f32 = 10.5;

#[derive(Clone)]
enum Kind {
    Text(String, Style),
    Image(usize, f32, f32),
}

#[derive(Clone)]
struct Piece {
    kind: Kind,
    width: f32,
    size: f32,
}

#[derive(Clone)]
struct Line {
    pieces: Vec<Piece>,
    width: f32,
    height: f32,
}

impl Line {
    fn new(size: f32) -> Self {
        Self {
            pieces: Vec::new(),
            width: 0.0,
            height: size * 1.65,
        }
    }
    fn add(&mut self, piece: Piece) {
        self.width += piece.width;
        self.pieces.push(piece);
    }
}

enum Op {
    Text {
        text: String,
        style: Style,
        size: f32,
        x: f32,
        y: f32,
        width: f32,
        color: Color,
    },
    Rect {
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        color: Color,
    },
    Image {
        id: usize,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
    },
}

pub struct Layout {
    fonts: Fonts,
    assets: Assets,
    theme: Theme,
    width: f32,
    height: f32,
    y: f32,
    pages: Vec<Vec<Op>>,
}

impl Layout {
    pub fn new(base: &Path, landscape: bool, theme: Theme) -> Result<Self> {
        let fonts = Fonts::new()?;
        let assets = Assets::new(base, &fonts);
        let (width, height) = if landscape {
            (841.89, 595.28)
        } else {
            (595.28, 841.89)
        };
        Ok(Self {
            fonts,
            assets,
            theme,
            width,
            height,
            y: TOP,
            pages: vec![Vec::new()],
        })
    }

    fn op(&mut self, op: Op) {
        self.pages.last_mut().unwrap().push(op);
    }
    fn bottom(&self) -> f32 {
        self.height - BOTTOM
    }
    fn capacity(&self) -> f32 {
        self.bottom() - TOP
    }
    fn new_page(&mut self) {
        self.pages.push(Vec::new());
        self.y = TOP;
    }
    fn ensure(&mut self, height: f32) {
        if self.y + height > self.bottom() + 0.01 && self.y > TOP + 0.01 {
            self.new_page();
        }
    }
    fn gap(&mut self, amount: f32) {
        self.y = (self.y + amount).min(self.bottom());
    }
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Color) {
        self.op(Op::Rect { x, y, w, h, color });
    }

    fn text_piece(&self, text: String, style: Style, size: f32) -> Piece {
        let size = if style.mono { size * 0.85 } else { size };
        Piece {
            width: self.fonts.width(&text, style.font(), size),
            kind: Kind::Text(text, style),
            size,
        }
    }

    // Wrap complete words first; split long URLs/code by Unicode scalar only when necessary.
    fn wrap(
        &mut self,
        spans: &[Inline],
        width: f32,
        size: f32,
        preserve: bool,
    ) -> Result<Vec<Line>> {
        let mut prepared = Vec::new();
        for span in spans {
            match span {
                Inline::Text(text, style) => {
                    for (text, font) in self.fonts.runs(text, style.font())? {
                        let mut style = style.clone();
                        style.font_override = Some(font);
                        prepared.push(Inline::Text(text, style));
                    }
                }
                _ => prepared.push(span.clone()),
            }
        }
        let mut lines = Vec::new();
        let mut line = Line::new(size);
        for span in &prepared {
            match span {
                Inline::Break => {
                    lines.push(line);
                    line = Line::new(size);
                }
                Inline::Image(src) => {
                    if !line.pieces.is_empty() {
                        lines.push(line);
                        line = Line::new(size);
                    }
                    let id = self.assets.load(src)?;
                    let (w, h) = self.assets.items[id].size();
                    let scale = (width / w).min((self.capacity() - 32.0) / h).min(0.75);
                    let (w, h) = (w * scale, h * scale);
                    let mut image_line = Line::new(size);
                    image_line.height = h + 16.0;
                    image_line.add(Piece {
                        kind: Kind::Image(id, w, h),
                        width: w,
                        size,
                    });
                    lines.push(image_line);
                }
                Inline::Text(text, style) => {
                    let text = text.replace('\t', "    ");
                    // Separate whitespace from words so styles can continue across inline nodes.
                    let mut chunks = Vec::new();
                    let mut start = 0;
                    let mut was_space = None;
                    for (pos, ch) in text.char_indices() {
                        let space = ch.is_whitespace();
                        if was_space.is_some_and(|old| old != space) {
                            chunks.push(&text[start..pos]);
                            start = pos;
                        }
                        was_space = Some(space);
                    }
                    if start < text.len() {
                        chunks.push(&text[start..]);
                    }
                    for chunk in chunks {
                        let whitespace = chunk.chars().all(char::is_whitespace);
                        if whitespace && !preserve && line.pieces.is_empty() {
                            continue;
                        }
                        let piece = self.text_piece(chunk.into(), style.clone(), size);
                        if line.width + piece.width > width + 0.01 && !line.pieces.is_empty() {
                            lines.push(line);
                            line = Line::new(size);
                            if whitespace && !preserve {
                                continue;
                            }
                        }
                        if piece.width <= width + 0.01 {
                            line.add(piece);
                        } else {
                            let mut part = String::new();
                            for ch in chunk.chars() {
                                let mut candidate = part.clone();
                                candidate.push(ch);
                                let candidate_piece =
                                    self.text_piece(candidate.clone(), style.clone(), size);
                                if line.width + candidate_piece.width > width && !part.is_empty() {
                                    line.add(self.text_piece(
                                        std::mem::take(&mut part),
                                        style.clone(),
                                        size,
                                    ));
                                    lines.push(line);
                                    line = Line::new(size);
                                }
                                part.push(ch);
                            }
                            if !part.is_empty() {
                                line.add(self.text_piece(part, style.clone(), size));
                            }
                        }
                    }
                }
            }
        }
        if !line.pieces.is_empty() || lines.is_empty() {
            lines.push(line);
        }
        Ok(lines)
    }

    fn draw_line(&mut self, line: &Line, x: f32, y: f32, color: Color, inline_bg: bool) {
        let mut cursor = x;
        for piece in &line.pieces {
            match &piece.kind {
                Kind::Text(text, style) => {
                    if style.mono && inline_bg {
                        self.rect(
                            cursor - 1.0,
                            y + 1.0,
                            piece.width + 2.0,
                            piece.size * 1.35,
                            self.theme.surface,
                        );
                    }
                    let color = if style.link.is_some() && inline_bg {
                        self.theme.accent
                    } else {
                        color
                    };
                    self.op(Op::Text {
                        text: text.clone(),
                        style: style.clone(),
                        size: piece.size,
                        x: cursor,
                        y: y + piece.size * 1.12,
                        width: piece.width,
                        color,
                    });
                }
                Kind::Image(id, w, h) => self.op(Op::Image {
                    id: *id,
                    x: cursor,
                    y: y + 8.0,
                    w: *w,
                    h: *h,
                }),
            }
            cursor += piece.width;
        }
    }

    fn paragraph(
        &mut self,
        spans: &[Inline],
        left: f32,
        right: f32,
        size: f32,
        color: Color,
    ) -> Result<()> {
        let width = self.width - left - right;
        let lines = self.wrap(spans, width, size, false)?;
        // Keep short paragraphs together; on longer paragraphs keep at least two lines at the start.
        let keep = if lines.len() <= 3 { lines.len() } else { 2 };
        self.ensure(lines.iter().take(keep).map(|l| l.height).sum());
        for line in &lines {
            self.ensure(line.height);
            let x = if matches!(line.pieces.first().map(|p| &p.kind), Some(Kind::Image(..))) {
                left + (width - line.width) / 2.0
            } else {
                left
            };
            self.draw_line(line, x, self.y, color, true);
            self.y += line.height;
        }
        self.gap(8.0);
        Ok(())
    }

    pub fn render(&mut self, blocks: &[Block]) -> Result<()> {
        self.blocks(blocks, LEFT, LEFT)
    }

    fn blocks(&mut self, blocks: &[Block], left: f32, right: f32) -> Result<()> {
        for block in blocks {
            match block {
                Block::Paragraph(spans) => {
                    self.paragraph(spans, left, right, SIZE, self.theme.text)?
                }
                Block::Heading(level, spans) => {
                    let size = match level {
                        1 => 29.0,
                        2 => 19.0,
                        3 => 14.0,
                        4 => 12.0,
                        _ => SIZE,
                    };
                    let spans: Vec<_> = spans
                        .iter()
                        .map(|span| match span {
                            Inline::Text(text, style) => {
                                let mut style = style.clone();
                                style.bold = true;
                                Inline::Text(text.clone(), style)
                            }
                            _ => span.clone(),
                        })
                        .collect();
                    let width = self.width - left - right;
                    let lines = self.wrap(&spans, width, size, false)?;
                    let height: f32 = lines.iter().map(|l| l.height).sum();
                    let before = if self.y > TOP { 18.0 } else { 0.0 };
                    self.ensure(before + height + 42.0);
                    if self.y > TOP {
                        self.gap(18.0);
                    }
                    for mut line in lines {
                        line.height = size * 1.3;
                        self.ensure(line.height + 24.0);
                        self.draw_line(&line, left, self.y, self.theme.heading, true);
                        self.y += line.height;
                    }
                    if *level <= 2 {
                        self.gap(7.0);
                        self.rect(
                            left,
                            self.y,
                            width,
                            if *level == 1 { 2.5 } else { 0.7 },
                            if *level == 1 {
                                self.theme.accent
                            } else {
                                self.theme.border
                            },
                        );
                        self.gap(11.0);
                    } else {
                        self.gap(7.0);
                    }
                }
                Block::Code(code) => {
                    let mut spans = Vec::new();
                    for (i, line) in code.trim_end_matches('\n').split('\n').enumerate() {
                        if i > 0 {
                            spans.push(Inline::Break);
                        }
                        spans.push(Inline::Text(
                            line.into(),
                            Style {
                                mono: true,
                                ..Default::default()
                            },
                        ));
                    }
                    let lines = self.wrap(&spans, self.width - left - right - 24.0, SIZE, true)?;
                    self.ensure(24.0 + lines.first().map_or(0.0, |l| l.height));
                    self.rect(
                        left,
                        self.y,
                        self.width - left - right,
                        10.0,
                        self.theme.surface,
                    );
                    self.y += 10.0;
                    for line in lines {
                        self.ensure(line.height);
                        self.rect(
                            left,
                            self.y,
                            self.width - left - right,
                            line.height,
                            self.theme.surface,
                        );
                        self.draw_line(&line, left + 12.0, self.y, self.theme.text, false);
                        self.y += line.height;
                    }
                    self.ensure(10.0);
                    self.rect(
                        left,
                        self.y,
                        self.width - left - right,
                        10.0,
                        self.theme.surface,
                    );
                    self.y += 18.0;
                }
                Block::Rule => {
                    self.ensure(22.0);
                    self.gap(8.0);
                    self.rect(
                        left,
                        self.y,
                        self.width - left - right,
                        0.7,
                        self.theme.border,
                    );
                    self.gap(14.0);
                }
                Block::Quote(children) => {
                    self.ensure(35.0);
                    let start_page = self.pages.len() - 1;
                    let start_y = self.y;
                    self.gap(5.0);
                    self.blocks(children, left + 15.0, right + 10.0)?;
                    for p in start_page..self.pages.len() {
                        let y = if p == start_page { start_y } else { TOP };
                        let end = if p == self.pages.len() - 1 {
                            self.y
                        } else {
                            self.bottom()
                        };
                        // Insert the quote's background before its content, including nested quotes.
                        let rect = Op::Rect {
                            x: left,
                            y,
                            w: self.width - left - right,
                            h: (end - y).max(0.0),
                            color: self.theme.surface,
                        };
                        let border = Op::Rect {
                            x: left,
                            y,
                            w: 2.5,
                            h: (end - y).max(0.0),
                            color: self.theme.accent,
                        };
                        self.pages[p].insert(0, border);
                        self.pages[p].insert(0, rect);
                    }
                    self.gap(6.0);
                }
                Block::List { start, items } => {
                    for (i, (checked, children)) in items.iter().enumerate() {
                        let first_height = if let Some(Block::Paragraph(spans)) = children.first() {
                            self.wrap(spans, self.width - left - 19.0 - right, SIZE, false)?
                                .iter()
                                .take(3)
                                .map(|l| l.height)
                                .sum()
                        } else {
                            SIZE * 3.3
                        };
                        self.ensure(first_height);
                        if let Some(checked) = checked {
                            let x = left + 2.0;
                            self.rect(x, self.y + 4.0, 8.0, 8.0, self.theme.accent);
                            self.rect(x + 1.0, self.y + 5.0, 6.0, 6.0, [255; 3]);
                            if *checked {
                                self.rect(x + 2.2, self.y + 6.2, 3.6, 3.6, self.theme.accent);
                            }
                        } else {
                            let label =
                                start.map_or_else(|| "•".into(), |start| format!("{}.", start + i));
                            let style = Style::default();
                            let width = self.fonts.width(&label, 0, SIZE);
                            self.op(Op::Text {
                                text: label,
                                style,
                                size: SIZE,
                                x: left,
                                y: self.y + SIZE * 1.12,
                                width,
                                color: self.theme.accent,
                            });
                        }
                        self.blocks(children, left + 19.0, right)?;
                    }
                }
                Block::Table(alignments, rows) => self.table(alignments, rows, left, right)?,
            }
        }
        Ok(())
    }

    fn table(
        &mut self,
        alignments: &[TableAlignment],
        rows: &[Vec<Vec<Inline>>],
        left: f32,
        right: f32,
    ) -> Result<()> {
        if rows.is_empty() || alignments.is_empty() {
            return Ok(());
        }
        let column = (self.width - left - right) / alignments.len() as f32;
        let mut wrapped = Vec::new();
        for (r, row) in rows.iter().enumerate() {
            let mut cells = Vec::new();
            for cell in row {
                let cell: Vec<_> = cell
                    .iter()
                    .map(|s| match s {
                        Inline::Text(text, style) if r == 0 => {
                            let mut s = style.clone();
                            s.bold = true;
                            Inline::Text(text.clone(), s)
                        }
                        _ => s.clone(),
                    })
                    .collect();
                cells.push(self.wrap(&cell, column - 16.0, SIZE * 0.92, false)?);
            }
            wrapped.push(cells);
        }
        let header_height = row_height(&wrapped[0]);
        if header_height + 32.0 > self.capacity() {
            return Err("The table header is too tall to fit on a page".into());
        }
        let first_body = wrapped.get(1).map_or(0.0, |row| row_height(row).min(40.0));
        self.ensure(header_height + first_body);
        for (r, cells) in wrapped.iter().enumerate() {
            let height = row_height(cells);
            if r > 0 && self.y + height > self.bottom() && height + header_height <= self.capacity()
            {
                self.new_page();
                self.table_row(&wrapped[0], alignments, left, column, header_height, 0);
            }
            if height + if r > 0 { header_height } else { 0.0 } <= self.capacity() {
                self.table_row(cells, alignments, left, column, height, r);
            } else {
                // Split oversized rows by complete lines, repeating the header on each continuation.
                let mut positions = vec![0; cells.len()];
                while positions
                    .iter()
                    .enumerate()
                    .any(|(c, p)| *p < cells[c].len())
                {
                    if self.bottom() - self.y < 34.0 {
                        self.new_page();
                        self.table_row(&wrapped[0], alignments, left, column, header_height, 0);
                    }
                    let available = self.bottom() - self.y - 16.0;
                    let mut slice = Vec::new();
                    for (c, cell) in cells.iter().enumerate() {
                        let mut part = Vec::new();
                        let mut used = 0.0;
                        while let Some(line) = cell.get(positions[c]) {
                            if used + line.height > available {
                                break;
                            }
                            used += line.height;
                            part.push(line.clone());
                            positions[c] += 1;
                        }
                        slice.push(part);
                    }
                    if slice.iter().all(Vec::is_empty) {
                        return Err(
                            "A table line or image exceeds the available page height".into()
                        );
                    }
                    let height = row_height(&slice);
                    self.table_row(&slice, alignments, left, column, height, r);
                }
            }
        }
        self.gap(12.0);
        Ok(())
    }

    fn table_row(
        &mut self,
        cells: &[Vec<Line>],
        alignments: &[TableAlignment],
        left: f32,
        column: f32,
        height: f32,
        row: usize,
    ) {
        let bg = if row == 0 {
            self.theme.primary
        } else if row.is_multiple_of(2) {
            self.theme.surface
        } else {
            [255; 3]
        };
        let color = if row == 0 {
            self.theme.on_primary
        } else {
            self.theme.text
        };
        for (c, cell) in cells.iter().enumerate() {
            let x = left + column * c as f32;
            self.rect(x, self.y, column, height, self.theme.border);
            self.rect(x + 0.4, self.y + 0.4, column - 0.8, height - 0.8, bg);
            let mut y = self.y + 8.0;
            for line in cell {
                let slack = (column - 16.0 - line.width).max(0.0);
                let offset = match alignments.get(c) {
                    Some(TableAlignment::Center) => slack / 2.0,
                    Some(TableAlignment::Right) => slack,
                    _ => 0.0,
                };
                self.draw_line(line, x + 8.0 + offset, y, color, row != 0);
                y += line.height;
            }
        }
        self.y += height;
    }

    pub fn finish(self, title: &str) -> Result<Vec<u8>> {
        let mut doc = Document::new();
        doc.set_metadata(
            Metadata::new()
                .title(title.into())
                .language("it".into())
                .creator("mdink (native)".into()),
        );
        let total = self.pages.len();
        for (index, ops) in self.pages.iter().enumerate() {
            let mut page =
                doc.start_page_with(PageSettings::from_wh(self.width, self.height).unwrap());
            let mut links = Vec::new();
            let mut surface = page.surface();
            for op in ops {
                match op {
                    Op::Rect { x, y, w, h, color } => {
                        let mut path = PathBuilder::new();
                        path.move_to(*x, *y);
                        path.line_to(x + w, *y);
                        path.line_to(x + w, y + h);
                        path.line_to(*x, y + h);
                        path.close();
                        surface.set_fill(Some(fill(*color)));
                        surface.set_stroke(None);
                        if let Some(path) = path.finish() {
                            surface.draw_path(&path);
                        }
                    }
                    Op::Text {
                        text,
                        style,
                        size,
                        x,
                        y,
                        width,
                        color,
                    } => {
                        surface.set_fill(Some(fill(*color)));
                        surface.set_stroke(None);
                        surface.draw_text(
                            Point::from_xy(*x, *y),
                            self.fonts.fonts[style.font()].clone(),
                            *size,
                            text,
                            false,
                            TextDirection::Auto,
                        );
                        if let Some(url) = &style.link
                            && (url.starts_with("https://")
                                || url.starts_with("http://")
                                || url.starts_with("mailto:"))
                        {
                            links.push((
                                Rect::from_xywh(*x, y - size, *width, size * 1.35).unwrap(),
                                url.clone(),
                            ));
                        }
                        if style.strike || style.link.is_some() {
                            let y = if style.strike {
                                y - size * 0.3
                            } else {
                                y + 1.6
                            };
                            let mut path = PathBuilder::new();
                            path.move_to(*x, y);
                            path.line_to(x + width, y);
                            path.line_to(x + width, y + 0.5);
                            path.line_to(*x, y + 0.5);
                            path.close();
                            surface.draw_path(&path.finish().unwrap());
                        }
                    }
                    Op::Image { id, x, y, w, h } => {
                        surface.push_transform(&Transform::from_translate(*x, *y));
                        let size = Size::from_wh(*w, *h).unwrap();
                        match &self.assets.items[*id] {
                            Asset::Raster(image) => surface.draw_image(image.clone(), size),
                            Asset::Svg(tree) => {
                                surface.draw_svg(tree, size, SvgSettings::default());
                            }
                        }
                        surface.pop();
                    }
                }
            }
            let footer = format!("{} / {total}", index + 1);
            let width = self.fonts.width(&footer, 0, 8.0);
            surface.set_fill(Some(fill(self.theme.muted)));
            surface.draw_text(
                Point::from_xy((self.width - width) / 2.0, self.height - 26.0),
                self.fonts.fonts[0].clone(),
                8.0,
                &footer,
                false,
                TextDirection::Auto,
            );
            surface.finish();
            for (rect, url) in links {
                page.add_annotation(Annotation::new_link(
                    LinkAnnotation::new(rect, Target::Action(LinkAction::new(url).into())),
                    None,
                ));
            }
            page.finish();
        }
        Ok(doc.finish()?)
    }
}

fn row_height(cells: &[Vec<Line>]) -> f32 {
    cells
        .iter()
        .map(|cell| cell.iter().map(|l| l.height).sum::<f32>())
        .fold(0.0, f32::max)
        + 16.0
}

fn fill(color: Color) -> Fill {
    Fill {
        paint: krilla::color::rgb::Color::new(color[0], color[1], color[2]).into(),
        ..Default::default()
    }
}
