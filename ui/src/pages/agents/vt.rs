//! Kleiner VT100/xterm-Interpreter in reinem Rust für das Terminal im Browser: Text, Cursor,
//! Löschen, Einfügen, Scrollbereiche, die üblichen SGR-Farben (16, 256, RGB), Zeilenumbruch,
//! Scrollback, alternativer Bildschirm, Bracketed Paste, Links (OSC 8) und Antworten auf
//! Statusabfragen. Beim Ändern der Grösse werden umgebrochene Zeilen neu umgebrochen.

use std::collections::VecDeque;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Color {
    #[default]
    Default,
    Idx(u8),
    Rgb(u8, u8, u8),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Attr {
    pub fg: Color,
    pub bg: Color,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cell {
    pub ch: char,
    pub attr: Attr,
    /// Link aus OSC 8: Nummer in `Vt::links` plus 1 (0 = keiner)
    pub link: u16,
}

impl Default for Cell {
    fn default() -> Self {
        Cell { ch: ' ', attr: Attr::default(), link: 0 }
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct Line {
    pub cells: Vec<Cell>,
    /// Fortsetzung der vorigen Zeile (automatischer Umbruch)
    pub wrapped: bool,
}

impl Line {
    fn blank(cols: usize, attr: Attr) -> Self {
        Line { cells: vec![Cell { ch: ' ', attr: Attr { bg: attr.bg, ..Attr::default() }, link: 0 }; cols], wrapped: false }
    }

    pub fn text(&self) -> String {
        self.cells.iter().map(|c| c.ch).collect::<String>().trim_end().to_string()
    }
}

#[derive(Clone, Debug, Default)]
enum State {
    #[default]
    Ground,
    Esc,
    Csi { params: String, private: Option<char>, inter: String },
    Osc(String),
    OscEsc(String),
    Charset,
}

#[derive(Clone, Copy, Debug, Default)]
struct Saved {
    x: usize,
    y: usize,
    attr: Attr,
}

/// Terminal-Zustand.
pub struct Vt {
    pub cols: usize,
    pub rows: usize,
    pub screen: Vec<Line>,
    pub scrollback: VecDeque<Line>,
    /// Hauptbildschirm, solange der alternative aktiv ist
    main: Option<Vec<Line>>,
    pub cx: usize,
    pub cy: usize,
    attr: Attr,
    saved: Saved,
    top: usize,
    bottom: usize,
    wrap_pending: bool,
    pub cursor_visible: bool,
    pub app_cursor: bool,
    pub bracketed_paste: bool,
    autowrap: bool,
    state: State,
    utf8: Vec<u8>,
    /// Antworten an das Programm (Statusabfragen), vom Aufrufer zu senden
    pub responses: String,
    pub title: String,
    /// Ziele der OSC-8-Links (nur http/https)
    pub links: Vec<String>,
    /// Aktueller Link für neue Zeichen
    link: u16,
    max_scrollback: usize,
}

impl Vt {
    pub fn new(cols: usize, rows: usize) -> Self {
        let cols = cols.max(2);
        let rows = rows.max(1);
        Vt {
            cols,
            rows,
            screen: (0..rows).map(|_| Line::blank(cols, Attr::default())).collect(),
            scrollback: VecDeque::new(),
            main: None,
            cx: 0,
            cy: 0,
            attr: Attr::default(),
            saved: Saved::default(),
            top: 0,
            bottom: rows - 1,
            wrap_pending: false,
            cursor_visible: true,
            app_cursor: false,
            bracketed_paste: false,
            autowrap: true,
            state: State::Ground,
            utf8: Vec::new(),
            responses: String::new(),
            title: String::new(),
            links: Vec::new(),
            link: 0,
            max_scrollback: 2000,
        }
    }

    /// Grösse ändern. Scrollback und Hauptbildschirm werden neu umgebrochen (nichts geht
    /// verloren, auch keine langen Adressen); der alternative Bildschirm wird nur angepasst,
    /// sein Programm zeichnet ihn ohnehin neu.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(2);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        if let Some(main) = self.main.take() {
            let (m, (y, x)) = self.reflow_main(main, (self.saved.y, self.saved.x), cols, rows);
            self.main = Some(m);
            self.saved.y = y;
            self.saved.x = x;
            for l in self.screen.iter_mut() {
                l.cells.resize(cols, Cell::default());
            }
            self.screen.resize(rows, Line::blank(cols, Attr::default()));
        } else {
            let screen = std::mem::take(&mut self.screen);
            let (s, (y, x)) = self.reflow_main(screen, (self.cy, self.cx.min(self.cols - 1)), cols, rows);
            self.screen = s;
            self.cy = y;
            self.cx = x;
        }
        self.cols = cols;
        self.rows = rows;
        self.top = 0;
        self.bottom = rows - 1;
        self.cx = self.cx.min(cols - 1);
        self.cy = self.cy.min(rows - 1);
        self.saved.x = self.saved.x.min(cols - 1);
        self.saved.y = self.saved.y.min(rows - 1);
        self.wrap_pending = false;
    }

    /// Scrollback und `screen` auf `cols` umbrechen; gibt den neuen Bildschirm (`rows` Zeilen)
    /// und die neue Position von `cursor` (Zeile, Spalte) darin zurück.
    fn reflow_main(&mut self, screen: Vec<Line>, cursor: (usize, usize), cols: usize, rows: usize) -> (Vec<Line>, (usize, usize)) {
        let sb = self.scrollback.len();
        let mut all: Vec<Line> = self.scrollback.drain(..).collect();
        all.extend(screen);
        let (mut all, marks) = reflow(all, cols, &[(sb + cursor.0, cursor.1), (sb, 0)]);
        let (cur, top) = (marks[0], marks[1]);
        // Oberkante bleibt, ausser der Cursor fiele unten heraus
        let start = if cur.0 >= top.0 + rows { cur.0 + 1 - rows } else { top.0 };
        all.truncate(start + rows);
        all.resize(start + rows, Line::blank(cols, Attr::default()));
        let screen = all.split_off(start);
        for l in all {
            self.push_scrollback(l);
        }
        (screen, (cur.0 - start, cur.1))
    }

    fn push_scrollback(&mut self, l: Line) {
        self.scrollback.push_back(l);
        while self.scrollback.len() > self.max_scrollback {
            self.scrollback.pop_front();
        }
    }

    /// Bytes vom Programm verarbeiten (UTF-8 über Stückgrenzen hinweg).
    pub fn write(&mut self, bytes: &[u8]) {
        let mut buf = std::mem::take(&mut self.utf8);
        buf.extend_from_slice(bytes);
        let mut i = 0;
        while i < buf.len() {
            let b = buf[i];
            let len = match b {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => {
                    self.char('\u{fffd}');
                    i += 1;
                    continue;
                }
            };
            if i + len > buf.len() {
                // Unvollständiges Zeichen: auf das nächste Stück warten
                self.utf8 = buf[i..].to_vec();
                return;
            }
            match std::str::from_utf8(&buf[i..i + len]) {
                Ok(s) => {
                    for c in s.chars() {
                        self.char(c);
                    }
                }
                Err(_) => self.char('\u{fffd}'),
            }
            i += len;
        }
    }

    fn char(&mut self, c: char) {
        match std::mem::take(&mut self.state) {
            State::Ground => self.ground(c),
            State::Esc => self.esc(c),
            State::Csi { mut params, mut private, mut inter } => match c {
                '0'..='9' | ';' | ':' => {
                    if params.len() < 64 {
                        params.push(c);
                    }
                    self.state = State::Csi { params, private, inter };
                }
                '?' | '>' | '<' | '=' | '!' if params.is_empty() => {
                    private = Some(c);
                    self.state = State::Csi { params, private, inter };
                }
                ' '..='/' => {
                    inter.push(c);
                    self.state = State::Csi { params, private, inter };
                }
                '@'..='~' => self.csi(c, &params, private, &inter),
                '\u{1b}' => self.state = State::Esc,
                _ => {}
            },
            State::Osc(mut s) => match c {
                '\u{7}' => self.osc(&s),
                '\u{1b}' => self.state = State::OscEsc(s),
                _ => {
                    if s.len() < 4096 {
                        s.push(c);
                    }
                    self.state = State::Osc(s);
                }
            },
            State::OscEsc(s) => {
                self.osc(&s);
                if c != '\\' {
                    self.char(c);
                }
            }
            State::Charset => {}
        }
    }

    fn osc(&mut self, s: &str) {
        if let Some(t) = s.strip_prefix("0;").or_else(|| s.strip_prefix("2;")) {
            self.title = t.chars().take(200).collect();
        } else if let Some(rest) = s.strip_prefix("8;") {
            // 8;Parameter;Adresse – eine leere Adresse beendet den Link
            let uri = rest.split_once(';').map(|(_, u)| u).unwrap_or("");
            self.link = self.link_id(uri);
        }
    }

    /// Nummer für ein Linkziel (0 = kein Link). Nur http/https, damit im Browser nichts
    /// anderes (z. B. `javascript:`) anklickbar wird.
    fn link_id(&mut self, uri: &str) -> u16 {
        let uri = uri.trim();
        if !(uri.starts_with("https://") || uri.starts_with("http://")) || uri.chars().any(char::is_control) {
            return 0;
        }
        let recent = self.links.len().saturating_sub(64);
        if let Some(i) = self.links[recent..].iter().position(|l| l == uri) {
            return (recent + i + 1) as u16;
        }
        if self.links.len() >= u16::MAX as usize {
            return 0;
        }
        self.links.push(uri.to_string());
        self.links.len() as u16
    }

    fn ground(&mut self, c: char) {
        match c {
            '\u{1b}' => self.state = State::Esc,
            '\r' => {
                self.cx = 0;
                self.wrap_pending = false;
            }
            '\n' | '\u{b}' | '\u{c}' => {
                self.wrap_pending = false;
                self.linefeed();
            }
            '\u{8}' => {
                self.wrap_pending = false;
                self.cx = self.cx.saturating_sub(1);
            }
            '\t' => {
                self.wrap_pending = false;
                self.cx = ((self.cx / 8 + 1) * 8).min(self.cols - 1);
            }
            '\u{7}' | '\u{e}' | '\u{f}' => {}
            c if (c as u32) < 0x20 || c == '\u{7f}' => {}
            c => self.put(c),
        }
    }

    fn esc(&mut self, c: char) {
        match c {
            '[' => self.state = State::Csi { params: String::new(), private: None, inter: String::new() },
            ']' => self.state = State::Osc(String::new()),
            '(' | ')' | '*' | '+' | '#' | '%' => self.state = State::Charset,
            '7' => self.save_cursor(),
            '8' => self.restore_cursor(),
            'D' => self.linefeed(),
            'E' => {
                self.cx = 0;
                self.linefeed();
            }
            'M' => self.reverse_index(),
            'c' => {
                let (cols, rows) = (self.cols, self.rows);
                let scrollback = std::mem::take(&mut self.scrollback);
                let links = std::mem::take(&mut self.links);
                *self = Vt::new(cols, rows);
                self.scrollback = scrollback;
                self.links = links;
            }
            _ => {}
        }
    }

    fn put(&mut self, c: char) {
        if self.wrap_pending && self.autowrap {
            self.cx = 0;
            self.linefeed();
            self.screen[self.cy].wrapped = true;
        }
        self.wrap_pending = false;
        let (x, y) = (self.cx.min(self.cols - 1), self.cy);
        self.screen[y].cells[x] = Cell { ch: c, attr: self.attr, link: self.link };
        if self.cx + 1 >= self.cols {
            self.wrap_pending = true;
        } else {
            self.cx += 1;
        }
    }

    fn blank(&self) -> Cell {
        Cell { ch: ' ', attr: Attr { bg: self.attr.bg, ..Attr::default() }, link: 0 }
    }

    fn blank_line(&self) -> Line {
        Line::blank(self.cols, self.attr)
    }

    fn linefeed(&mut self) {
        if self.cy == self.bottom {
            self.scroll_up(1);
        } else if self.cy + 1 < self.rows {
            self.cy += 1;
        }
    }

    fn reverse_index(&mut self) {
        if self.cy == self.top {
            self.scroll_down(1);
        } else {
            self.cy = self.cy.saturating_sub(1);
        }
    }

    fn scroll_up(&mut self, n: usize) {
        for _ in 0..n.min(self.rows) {
            let l = self.screen.remove(self.top);
            if self.top == 0 && self.main.is_none() {
                self.push_scrollback(l);
            }
            let b = self.blank_line();
            self.screen.insert(self.bottom, b);
        }
    }

    fn scroll_down(&mut self, n: usize) {
        for _ in 0..n.min(self.rows) {
            self.screen.remove(self.bottom);
            let b = self.blank_line();
            self.screen.insert(self.top, b);
        }
    }

    fn save_cursor(&mut self) {
        self.saved = Saved { x: self.cx, y: self.cy, attr: self.attr };
    }

    fn restore_cursor(&mut self) {
        self.cx = self.saved.x.min(self.cols - 1);
        self.cy = self.saved.y.min(self.rows - 1);
        self.attr = self.saved.attr;
        self.wrap_pending = false;
    }

    fn csi(&mut self, f: char, params: &str, private: Option<char>, inter: &str) {
        let p: Vec<u32> = params.split([';', ':']).map(|x| x.parse().unwrap_or(0)).collect();
        let arg = |i: usize, d: u32| -> usize {
            let v = p.get(i).copied().unwrap_or(0);
            (if v == 0 { d } else { v }) as usize
        };
        if !inter.is_empty() {
            return; // z. B. DECSCUSR (Cursorform) – ohne Bedeutung hier
        }
        self.wrap_pending = false;
        match (private, f) {
            (None, 'A') => self.cy = self.cy.saturating_sub(arg(0, 1)).max(if self.cy >= self.top { self.top } else { 0 }),
            (None, 'B') | (None, 'e') => self.cy = (self.cy + arg(0, 1)).min(if self.cy <= self.bottom { self.bottom } else { self.rows - 1 }),
            (None, 'C') | (None, 'a') => self.cx = (self.cx + arg(0, 1)).min(self.cols - 1),
            (None, 'D') => self.cx = self.cx.saturating_sub(arg(0, 1)),
            (None, 'E') => {
                self.cy = (self.cy + arg(0, 1)).min(self.rows - 1);
                self.cx = 0;
            }
            (None, 'F') => {
                self.cy = self.cy.saturating_sub(arg(0, 1));
                self.cx = 0;
            }
            (None, 'G') | (None, '`') => self.cx = (arg(0, 1) - 1).min(self.cols - 1),
            (None, 'H') | (None, 'f') => {
                self.cy = (arg(0, 1) - 1).min(self.rows - 1);
                self.cx = (arg(1, 1) - 1).min(self.cols - 1);
            }
            (None, 'd') => self.cy = (arg(0, 1) - 1).min(self.rows - 1),
            (None, 'J') => {
                let b = self.blank();
                match p.first().copied().unwrap_or(0) {
                    0 => {
                        for x in self.cx..self.cols {
                            self.screen[self.cy].cells[x] = b;
                        }
                        for y in self.cy + 1..self.rows {
                            self.screen[y] = self.blank_line();
                        }
                    }
                    1 => {
                        for x in 0..=self.cx.min(self.cols - 1) {
                            self.screen[self.cy].cells[x] = b;
                        }
                        for y in 0..self.cy {
                            self.screen[y] = self.blank_line();
                        }
                    }
                    2 | 3 => {
                        // Bildschirm löschen: bisherigen Inhalt im Scrollback behalten
                        if self.main.is_none() && p.first() == Some(&2) {
                            let last = self.screen.iter().rposition(|l| !l.text().is_empty());
                            if let Some(last) = last {
                                let lines: Vec<Line> = self.screen[..=last].to_vec();
                                for l in lines {
                                    self.push_scrollback(l);
                                }
                            }
                        }
                        if p.first() == Some(&3) {
                            self.scrollback.clear();
                        }
                        for y in 0..self.rows {
                            self.screen[y] = self.blank_line();
                        }
                    }
                    _ => {}
                }
            }
            (None, 'K') => {
                let b = self.blank();
                let y = self.cy;
                let range = match p.first().copied().unwrap_or(0) {
                    0 => self.cx..self.cols,
                    1 => 0..(self.cx + 1).min(self.cols),
                    _ => 0..self.cols,
                };
                for x in range {
                    self.screen[y].cells[x] = b;
                }
            }
            (None, 'L') => {
                if self.cy >= self.top && self.cy <= self.bottom {
                    for _ in 0..arg(0, 1).min(self.bottom - self.cy + 1) {
                        self.screen.remove(self.bottom);
                        let b = self.blank_line();
                        self.screen.insert(self.cy, b);
                    }
                }
            }
            (None, 'M') => {
                if self.cy >= self.top && self.cy <= self.bottom {
                    for _ in 0..arg(0, 1).min(self.bottom - self.cy + 1) {
                        self.screen.remove(self.cy);
                        let b = self.blank_line();
                        self.screen.insert(self.bottom, b);
                    }
                }
            }
            (None, 'P') => {
                let n = arg(0, 1).min(self.cols - self.cx);
                let b = self.blank();
                let cells = &mut self.screen[self.cy].cells;
                cells.drain(self.cx..self.cx + n);
                cells.extend(std::iter::repeat_n(b, n));
            }
            (None, '@') => {
                let n = arg(0, 1).min(self.cols - self.cx);
                let b = self.blank();
                let cols = self.cols;
                let cells = &mut self.screen[self.cy].cells;
                for _ in 0..n {
                    cells.insert(self.cx, b);
                }
                cells.truncate(cols);
            }
            (None, 'X') => {
                let b = self.blank();
                for x in self.cx..(self.cx + arg(0, 1)).min(self.cols) {
                    self.screen[self.cy].cells[x] = b;
                }
            }
            (None, 'S') => self.scroll_up(arg(0, 1)),
            (None, 'T') => self.scroll_down(arg(0, 1)),
            (None, 'm') => self.sgr(&p),
            (None, 'r') => {
                let top = arg(0, 1) - 1;
                let bottom = arg(1, self.rows as u32) - 1;
                if top < bottom && bottom < self.rows {
                    self.top = top;
                    self.bottom = bottom;
                } else {
                    self.top = 0;
                    self.bottom = self.rows - 1;
                }
                self.cx = 0;
                self.cy = 0;
            }
            (None, 's') => self.save_cursor(),
            (None, 'u') => self.restore_cursor(),
            (None, 'n') => match p.first().copied().unwrap_or(0) {
                5 => self.responses.push_str("\u{1b}[0n"),
                6 => self.responses.push_str(&format!("\u{1b}[{};{}R", self.cy + 1, self.cx + 1)),
                _ => {}
            },
            (None, 'c') => self.responses.push_str("\u{1b}[?1;2c"),
            (Some('>'), 'c') => self.responses.push_str("\u{1b}[>0;10;0c"),
            (Some('?'), 'h') | (Some('?'), 'l') => {
                let on = f == 'h';
                for mode in &p {
                    match mode {
                        1 => self.app_cursor = on,
                        7 => self.autowrap = on,
                        25 => self.cursor_visible = on,
                        47 | 1047 | 1049 => self.set_alt(on, *mode == 1049),
                        2004 => self.bracketed_paste = on,
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn set_alt(&mut self, on: bool, save_cursor: bool) {
        if on && self.main.is_none() {
            if save_cursor {
                self.save_cursor();
            }
            let blank: Vec<Line> = (0..self.rows).map(|_| Line::blank(self.cols, Attr::default())).collect();
            self.main = Some(std::mem::replace(&mut self.screen, blank));
            self.cx = 0;
            self.cy = 0;
        } else if !on {
            if let Some(m) = self.main.take() {
                self.screen = m;
                if save_cursor {
                    self.restore_cursor();
                }
            }
        }
        self.top = 0;
        self.bottom = self.rows - 1;
    }

    fn sgr(&mut self, p: &[u32]) {
        if p.is_empty() {
            self.attr = Attr::default();
            return;
        }
        let mut i = 0;
        while i < p.len() {
            match p[i] {
                0 => self.attr = Attr::default(),
                1 => self.attr.bold = true,
                2 => self.attr.dim = true,
                3 => self.attr.italic = true,
                4 => self.attr.underline = true,
                7 => self.attr.inverse = true,
                22 => {
                    self.attr.bold = false;
                    self.attr.dim = false;
                }
                23 => self.attr.italic = false,
                24 => self.attr.underline = false,
                27 => self.attr.inverse = false,
                n @ 30..=37 => self.attr.fg = Color::Idx((n - 30) as u8),
                n @ 40..=47 => self.attr.bg = Color::Idx((n - 40) as u8),
                n @ 90..=97 => self.attr.fg = Color::Idx((n - 90 + 8) as u8),
                n @ 100..=107 => self.attr.bg = Color::Idx((n - 100 + 8) as u8),
                39 => self.attr.fg = Color::Default,
                49 => self.attr.bg = Color::Default,
                n @ (38 | 48) => {
                    let color = match p.get(i + 1) {
                        Some(5) => {
                            let c = p.get(i + 2).copied().unwrap_or(0).min(255) as u8;
                            i += 2;
                            Some(Color::Idx(c))
                        }
                        Some(2) => {
                            let g = |k: usize| p.get(i + k).copied().unwrap_or(0).min(255) as u8;
                            let c = Color::Rgb(g(2), g(3), g(4));
                            i += 4;
                            Some(c)
                        }
                        _ => None,
                    };
                    if let Some(c) = color {
                        if n == 38 {
                            self.attr.fg = c;
                        } else {
                            self.attr.bg = c;
                        }
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }

    /// Alle Zeilen zum Anzeigen: Scrollback (nicht im alternativen Bildschirm) und Bildschirm.
    pub fn lines(&self) -> Vec<&Line> {
        let mut v: Vec<&Line> = Vec::with_capacity(self.scrollback.len() + self.rows);
        if self.main.is_none() {
            v.extend(self.scrollback.iter());
        }
        v.extend(self.screen.iter());
        v
    }

    /// Index der ersten Bildschirmzeile in `lines()`.
    pub fn screen_offset(&self) -> usize {
        if self.main.is_none() {
            self.scrollback.len()
        } else {
            0
        }
    }
}

/// Zeilen auf `cols` neu umbrechen: Zeilen mit `wrapped` werden zur logischen Zeile
/// zusammengefügt und neu aufgeteilt. `marks` (Zeile, Spalte) wandern mit.
fn reflow(lines: Vec<Line>, cols: usize, marks: &[(usize, usize)]) -> (Vec<Line>, Vec<(usize, usize)>) {
    let mut out: Vec<Line> = Vec::with_capacity(lines.len());
    let mut moved = vec![(0, 0); marks.len()];
    let mut i = 0;
    while i < lines.len() {
        let mut j = i + 1;
        while j < lines.len() && lines[j].wrapped {
            j += 1;
        }
        let mut cells: Vec<Cell> = Vec::new();
        let mut starts = Vec::with_capacity(j - i);
        for l in &lines[i..j] {
            starts.push(cells.len());
            cells.extend_from_slice(&l.cells);
        }
        // Leere Zellen am Ende weglassen, aber nicht vor einer Markierung (Cursor)
        let mut len = cells.iter().rposition(|c| *c != Cell::default()).map_or(0, |p| p + 1);
        let mine: Vec<(usize, usize)> = marks
            .iter()
            .enumerate()
            .filter(|(_, m)| (i..j).contains(&m.0))
            .map(|(k, m)| (k, starts[m.0 - i] + m.1))
            .collect();
        for &(_, off) in &mine {
            len = len.max(off + 1);
        }
        cells.truncate(len);
        let base = out.len();
        for (k, chunk) in cells.chunks(cols).enumerate() {
            let mut c = chunk.to_vec();
            c.resize(cols, Cell::default());
            out.push(Line { cells: c, wrapped: k > 0 });
        }
        if len == 0 {
            out.push(Line::blank(cols, Attr::default()));
        }
        for (k, off) in mine {
            moved[k] = (base + off / cols, off % cols);
        }
        i = j;
    }
    (out, moved)
}

/// Zeichen, die in einer Adresse vorkommen (für Adressen über harte Zeilenwechsel).
fn url_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "-._~:/?#[]@!$&()*+,;=%".contains(c)
}

/// Adressen in den Zeilen: Ziele der OSC-8-Links und Text, der wie eine Adresse aussieht –
/// auch über umgebrochene Zeilen hinweg. Gibt die Adressen und je Zelle den Index darin zurück.
pub fn find_links(lines: &[&Line], links: &[String]) -> (Vec<String>, Vec<Vec<Option<usize>>>) {
    let mut urls: Vec<String> = Vec::new();
    let mut link_of: Vec<Vec<Option<usize>>> = lines.iter().map(|l| vec![None; l.cells.len()]).collect();
    // 1. OSC 8: das Ziel steht vollständig im Link, egal wie die Zeilen aussehen
    for (li, l) in lines.iter().enumerate() {
        for (ci, c) in l.cells.iter().enumerate() {
            let Some(u) = c.link.checked_sub(1).and_then(|i| links.get(i as usize)) else { continue };
            let idx = match urls.iter().position(|x| x == u) {
                Some(i) => i,
                None => {
                    urls.push(u.clone());
                    urls.len() - 1
                }
            };
            link_of[li][ci] = Some(idx);
        }
    }
    // 2. Text: alle Zeichen als Strom. Nach einer vollen Zeile ohne Umbruch-Kennung steht ein
    // weicher Trenner ('\0'), sonst ein Zeilenwechsel – Programme wie Ink brechen selbst um.
    let mut stream: Vec<(char, Option<(usize, usize)>)> = Vec::new();
    for (li, l) in lines.iter().enumerate() {
        for (ci, c) in l.cells.iter().enumerate() {
            stream.push((c.ch, Some((li, ci))));
        }
        match lines.get(li + 1) {
            Some(next) if next.wrapped => {}
            _ if l.cells.last().is_some_and(|c| c.ch != ' ') => stream.push(('\0', None)),
            _ => stream.push(('\n', None)),
        }
    }
    let starts_at = |k: usize, p: &str| p.chars().enumerate().all(|(n, c)| stream.get(k + n).is_some_and(|s| s.0 == c && s.1.is_some()));
    let mut k = 0;
    while k < stream.len() {
        let linked = stream[k].1.is_some_and(|(li, ci)| link_of[li][ci].is_some());
        if linked || !(starts_at(k, "https://") || starts_at(k, "http://")) {
            k += 1;
            continue;
        }
        let mut e = k;
        let mut url = String::new();
        let mut cells: Vec<(usize, usize)> = Vec::new();
        while e < stream.len() {
            let (c, pos) = stream[e];
            match pos {
                None if c == '\0' && stream.get(e + 1).is_some_and(|n| n.1.is_some() && url_char(n.0)) => {}
                None => break,
                Some((li, ci)) => {
                    if c.is_whitespace() || matches!(c, '"' | '\'' | '<' | '>' | '`') || link_of[li][ci].is_some() {
                        break;
                    }
                    url.push(c);
                    cells.push((li, ci));
                }
            }
            e += 1;
        }
        while url.ends_with(['.', ',', ')', ']', ';', ':']) {
            url.pop();
            cells.pop();
        }
        if url.len() > 10 {
            let idx = urls.len();
            urls.push(url);
            for (li, ci) in cells {
                link_of[li][ci] = Some(idx);
            }
        }
        k = e.max(k + 1);
    }
    (urls, link_of)
}

/// Farbe als CSS. Die 16 Grundfarben kommen aus `--term-0` … `--term-15` (`tailwind.css`: dunkel,
/// im eingefärbten Terminal des hellen Themas dunklere).
pub fn css(c: Color) -> Option<String> {
    match c {
        Color::Default => None,
        Color::Idx(i) if i < 16 => Some(format!("var(--term-{i})")),
        Color::Idx(i) if i < 232 => {
            let i = i - 16;
            let v = |x: u8| if x == 0 { 0 } else { 55 + x as u32 * 40 };
            Some(format!("rgb({},{},{})", v(i / 36), v((i / 6) % 6), v(i % 6)))
        }
        Color::Idx(i) => {
            let g = 8 + (i as u32 - 232) * 10;
            Some(format!("rgb({g},{g},{g})"))
        }
        Color::Rgb(r, g, b) => Some(format!("rgb({r},{g},{b})")),
    }
}

/// Stil einer Zelle als CSS (leer = Vorgabe).
pub fn style(a: &Attr) -> String {
    let (mut fg, mut bg) = (css(a.fg), css(a.bg));
    if a.inverse {
        let f = fg.take().unwrap_or_else(|| "var(--background)".into());
        let b = bg.take().unwrap_or_else(|| "var(--foreground)".into());
        fg = Some(b);
        bg = Some(f);
    }
    let mut s = String::new();
    if let Some(f) = fg {
        s.push_str(&format!("color:{f};"));
    }
    if let Some(b) = bg {
        s.push_str(&format!("background:{b};"));
    }
    if a.bold {
        s.push_str("font-weight:600;");
    }
    if a.dim {
        s.push_str("opacity:.65;");
    }
    if a.italic {
        s.push_str("font-style:italic;");
    }
    if a.underline {
        s.push_str("text-decoration:underline;");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(vt: &Vt) -> Vec<String> {
        vt.screen.iter().map(|l| l.text()).collect()
    }

    #[test]
    fn prints_and_wraps() {
        let mut vt = Vt::new(5, 3);
        vt.write(b"hallo welt\r\nzwei");
        assert_eq!(text(&vt), vec!["hallo", " welt", "zwei"]);
        assert!(vt.screen[1].wrapped);
        vt.write(b"\r\nmehr");
        assert_eq!(vt.scrollback.len(), 1);
        assert_eq!(text(&vt)[2], "mehr");
    }

    #[test]
    fn cursor_erase_and_colors() {
        let mut vt = Vt::new(10, 2);
        vt.write(b"abcdef\x1b[3D\x1b[K\x1b[1;31mX\x1b[0m");
        assert_eq!(text(&vt)[0], "abcX");
        assert_eq!(vt.screen[0].cells[3].attr.fg, Color::Idx(1));
        assert!(vt.screen[0].cells[3].attr.bold);
        vt.write(b"\x1b[2;1H\x1b[38;5;196mY\x1b[48;2;1;2;3mZ");
        assert_eq!(vt.screen[1].cells[0].attr.fg, Color::Idx(196));
        assert_eq!(vt.screen[1].cells[1].attr.bg, Color::Rgb(1, 2, 3));
        vt.write(b"\x1b[6n");
        assert_eq!(vt.responses, "\x1b[2;3R");
    }

    #[test]
    fn shrink_keeps_output_on_screen() {
        let mut vt = Vt::new(10, 6);
        vt.write(b"a\r\nb\r\nc");
        vt.resize(10, 4);
        assert!(vt.scrollback.is_empty());
        assert_eq!(text(&vt), vec!["a", "b", "c", ""]);
        vt.resize(10, 2);
        assert_eq!(vt.scrollback.len(), 1);
        assert_eq!(text(&vt), vec!["b", "c"]);
    }

    #[test]
    fn utf8_split_and_alt_screen() {
        let mut vt = Vt::new(10, 2);
        let s = "äö".as_bytes();
        vt.write(&s[..1]);
        vt.write(&s[1..]);
        assert_eq!(text(&vt)[0], "äö");
        vt.write(b"\x1b[?1049hTUI\x1b[?1049l");
        assert_eq!(text(&vt)[0], "äö");
        vt.write(b"\x1b]0;Titel\x07\x1b[?2004h");
        assert_eq!(vt.title, "Titel");
        assert!(vt.bracketed_paste);
    }

    const LOGIN: &str = "https://claude.com/cai/oauth/authorize?code=true&client_id=9d1c250a-e61b-44d9-88ed-5944d1962f5e&response_type=code&redirect_uri=https%3A%2F%2Fplatform.claude.com%2Foauth%2Fcode%2Fcallback&scope=org%3Acreate_api_key+user%3Aprofile+user%3Ainference+user%3Asessions%3Aclaude_code&code_challenge=Zx1_2bWq9kXcQmVt8yRr4LpNaE3oHs7uJdF6gT0iYwA&code_challenge_method=S256&state=q8Vh2mXo5RtP1cKe9LsWbN3yAf7jDg4uZiQ0Ex6OlTs";

    fn urls(vt: &Vt) -> Vec<String> {
        find_links(&vt.lines(), &vt.links).0
    }

    #[test]
    fn osc8_login_link_survives_resize() {
        // So gibt `claude auth login` den Link aus (OSC 8 mit BEL)
        let out = format!("Opening browser to sign in\u{2026}\r\nIf the browser didn't open, visit: \x1b]8;;{LOGIN}\x07{LOGIN}\x1b]8;;\x07\r\nPaste code here if prompted > ");
        let mut vt = Vt::new(100, 30);
        for chunk in out.as_bytes().chunks(7) {
            vt.write(chunk);
        }
        assert_eq!(urls(&vt), vec![LOGIN.to_string()]);
        for (cols, rows) in [(98, 30), (61, 12), (40, 8), (120, 40), (100, 30)] {
            vt.resize(cols, rows);
            assert_eq!(urls(&vt), vec![LOGIN.to_string()], "{cols}x{rows}");
            let joined: String = vt.lines().iter().map(|l| l.text()).collect::<Vec<_>>().concat();
            assert!(joined.contains(LOGIN), "Text nach {cols}x{rows} unvollständig");
            assert!(vt.lines().iter().all(|l| l.cells.len() == cols));
        }
        // Cursor steht weiter hinter der Eingabeaufforderung
        assert_eq!(vt.screen[vt.cy].text(), "Paste code here if prompted >");
        assert_eq!(vt.cx, "Paste code here if prompted > ".len());
    }

    #[test]
    fn text_link_survives_shrink_and_hard_breaks() {
        let mut vt = Vt::new(100, 30);
        vt.write(format!("visit: {LOGIN}\r\n$ ").as_bytes());
        vt.resize(97, 30);
        assert_eq!(urls(&vt), vec![LOGIN.to_string()]);
        // Programm bricht selbst um (volle Zeilen, dann \r\n)
        let mut vt = Vt::new(40, 20);
        let chars: Vec<char> = LOGIN.chars().collect();
        let lines: Vec<String> = chars.chunks(40).map(|c| c.iter().collect()).collect();
        vt.write(format!("{}\r\nweiter", lines.join("\r\n")).as_bytes());
        assert_eq!(urls(&vt), vec![LOGIN.to_string()]);
    }

    #[test]
    fn osc8_only_web_links() {
        let mut vt = Vt::new(40, 3);
        vt.write(b"\x1b]8;;javascript:alert(1)\x1b\\klick\x1b]8;;\x1b\\ \x1b]8;id=1;https://example.com/a\x1b\\hier\x1b]8;;\x1b\\");
        assert_eq!(vt.links, vec!["https://example.com/a".to_string()]);
        assert_eq!(vt.screen[0].cells[0].link, 0);
        assert_eq!(vt.screen[0].cells[6].link, 1);
        assert_eq!(vt.screen[0].cells[10].link, 0);
        assert_eq!(urls(&vt), vec!["https://example.com/a".to_string()]);
    }
}
