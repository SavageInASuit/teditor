use std::env;
use std::error::Error;
use std::fs;
use std::io;
use std::io::{Read, Write};
use std::path::PathBuf;
use termios::*;

const VERSION: &str = "0.0.1";

const CLEAR_SCREEN: &str = "\x1b[2J";
const CLEAR_LINE: &str = "\x1b[K";
const HIDE_CURSOR: &str = "\x1b[?25l";
const SHOW_CURSOR: &str = "\x1b[?25h";

struct Erow {
    size: u16,
    chars: String,
}

struct EditorConfig {
    orig_termios: Termios,
    screen_rows: u16,
    screen_cols: u16,
    cursor_x: u16,
    cursor_y: u16,
    num_rows: u16,
    rows: Vec<Erow>,
    row_offset: u16,
    col_offset: u16,
}

#[repr(u32)]
enum EditorKey {
    Left = 1000,
    Right = 1001,
    Up = 1002,
    Down = 1003,
    PageUp = 1004,
    PageDown = 1005,
    Home = 1006,
    End = 1007,
    Delete = 1008,
}

impl TryFrom<u32> for EditorKey {
    type Error = String;

    fn try_from(key: u32) -> Result<Self, Self::Error> {
        if ((key as u8) as u32) == key {
            // In ascii range
            match key as u8 {
                b'h' | b'D' => Ok(EditorKey::Left),
                b'l' | b'C' => Ok(EditorKey::Right),
                b'k' | b'A' => Ok(EditorKey::Up),
                b'j' | b'B' => Ok(EditorKey::Down),
                _ => Err(format!("Unknown key: {}", key)),
            }
        } else {
            match key {
                x if x == EditorKey::Left as u32 => Ok(EditorKey::Left),
                x if x == EditorKey::Right as u32 => Ok(EditorKey::Right),
                x if x == EditorKey::Up as u32 => Ok(EditorKey::Up),
                x if x == EditorKey::Down as u32 => Ok(EditorKey::Down),
                _ => Err(format!("Unknown key: {}", key)),
            }
        }
    }
}

struct ScreenBuffer {
    to_print: String,
}
impl ScreenBuffer {
    fn new() -> ScreenBuffer {
        ScreenBuffer {
            to_print: String::new(),
        }
    }

    fn append(&mut self, s: &str) {
        self.to_print.push_str(s);
    }

    fn flush(&mut self) {
        print!("{}", self.to_print);
        self.to_print.clear();
    }
}

fn disable_raw_mode(orig_termios: &Termios) {
    let fd = 0;
    match termios::tcsetattr(fd, TCSAFLUSH, orig_termios) {
        Ok(_) => (),
        Err(e) => die("setting termios disable raw mode", &Some(Box::new(e))),
    }
}

fn enable_raw_mode(t: &mut Termios) {
    t.c_iflag &= !(BRKINT | ICRNL | INPCK | ISTRIP | IXON);
    t.c_oflag &= !(OPOST);
    t.c_cflag &= !(CS8);
    t.c_lflag &= !(ECHO | ICANON | IEXTEN | ISIG);

    t.c_cc[VMIN] = 0;
    t.c_cc[VTIME] = 1;

    match termios::tcsetattr(0, TCSAFLUSH, t) {
        Ok(_) => (),
        Err(e) => die("setting termios raw mode", &Some(Box::new(e))),
    }
}

fn setup_terminal() -> Termios {
    let fd = 0;
    let mut t = match Termios::from_fd(fd) {
        Ok(t) => t,
        Err(e) => {
            die("getting termios", &Some(Box::new(e)));
            panic!("Shouldn't get here");
        }
    };
    let orig_termios = t;
    enable_raw_mode(&mut t);
    orig_termios
}

fn ctrl_key(key: char) -> u32 {
    key as u32 & 0x1f
}

fn is_cntrl(key: u32) -> bool {
    key < 32
}

fn die(e: &str, err: &Option<Box<dyn Error>>) {
    // disable_raw_mode(&Termios::from_fd(0).unwrap());
    clear_and_reset_cursor(None);

    if let Some(err) = err.as_ref() {
        panic!("{}: {}", e, err);
    } else {
        panic!("{}", e);
    }
}

// todo refactor this
fn get_cursor_position() -> (u16, u16) {
    println!("\x1b[6n");
    let mut buf: [u8; 16] = [0; 16];
    match io::stdin().read(&mut buf) {
        Ok(_) => (),
        Err(e) => die("reading cursor position", &Some(Box::new(e))),
    }

    // check for start of escape sequence
    if buf[0] != 27 || buf[1] != 91 {
        die(
            format!("unexpected output reading cursor position: {:?}", buf).as_str(),
            &None,
        );
        (0, 0)
    } else {
        // parse the position
        let mut row: u16 = 0;
        let mut col: u16 = 0;
        let mut i = 2;
        while i < buf.len() {
            if buf[i] == 59 {
                i += 1;
                break;
            }
            row = row * 10 + (buf[i] as u16 - 48);
            i += 1;
        }
        while i < buf.len() {
            if buf[i] == 82 {
                break;
            }
            col = col * 10 + (buf[i] as u16 - 48);
            i += 1;
        }
        (col, row)
    }
    // get the cursor position buffer
}

fn get_window_size() -> (u16, u16) {
    if let Some(size) = termsize::get() {
        (size.cols, size.rows)
    } else {
        print!("\x1b[999C\x1b[999B");
        get_cursor_position()
    }
}

// IO
fn editor_open(editor: &mut EditorConfig, path: &str) {
    let path = PathBuf::from(path);
    let file_result = fs::read_to_string(path);
    let file_content = match file_result {
        Ok(content) => content,
        Err(e) => {
            die(
                format!("Error when trying to load file: {}", e).as_str(),
                &None,
            );
            panic!("Shouldn't get here");
        }
    };
    for line in file_content.lines() {
        let linelen = line.len() as u16;
        let row = Erow {
            size: linelen,
            chars: line.to_string(),
        };
        editor.rows.push(row);
        editor.num_rows += 1;
    }
}

fn toggle_cursor(sb: &mut ScreenBuffer, show: bool) {
    if show {
        sb.append(SHOW_CURSOR);
    } else {
        sb.append(HIDE_CURSOR);
    }
}

fn set_cursor_position(sb: Option<&mut ScreenBuffer>, row: u16, col: u16) {
    match sb {
        Some(sb) => sb.append(format!("\x1b[{};{}H", row, col).as_str()),
        None => print!("\x1b[{};{}H", row, col),
    }
}

fn move_cursor(editor: &mut EditorConfig, key: EditorKey) {
    match key {
        EditorKey::Left => {
            if editor.cursor_x > 0 {
                editor.cursor_x -= 1;
            }
        }
        EditorKey::Right => {
            editor.cursor_x += 1;
        }
        EditorKey::Up => {
            if editor.cursor_y > 0 {
                editor.cursor_y -= 1;
            }
        }
        EditorKey::Down => {
            if editor.cursor_y < editor.num_rows {
                editor.cursor_y += 1;
            }
        }
        EditorKey::PageUp => {
            editor.cursor_y = 0;
        }
        EditorKey::PageDown => {
            editor.cursor_y = editor.screen_rows - 1;
        }
        EditorKey::Home => {
            editor.cursor_x = 0;
        }
        EditorKey::End => {
            editor.cursor_x = editor.screen_cols - 1;
        }
        _ => (),
    }
}

fn clear_and_reset_cursor(sb: Option<&mut ScreenBuffer>) {
    if let Some(buf) = sb {
        buf.append(CLEAR_SCREEN);
        set_cursor_position(Some(buf), 1, 1);
    } else {
        print!("{}", CLEAR_SCREEN);
        set_cursor_position(None, 1, 1);
    }

    match io::stdout().flush() {
        Ok(_) => (),
        Err(e) => die(
            "flushing stdout after clear and reset cursor",
            &Some(Box::new(e)),
        ),
    }
}

fn editor_draw_rows(sb: &mut ScreenBuffer, editor: &EditorConfig) {
    for y in 0..editor.screen_rows {
        let file_row = y + editor.row_offset;
        if file_row >= editor.num_rows {
            if editor.num_rows == 0 && y == editor.screen_rows / 3 {
                // TODO: extract this into a function
                let welcome = format!("Teditor -- version {}", VERSION);
                if welcome.len() > editor.screen_cols as usize {
                    sb.append(&welcome[..(editor.screen_cols as usize)]);
                } else {
                    let mut padding = ((editor.screen_cols as usize) - welcome.len()) / 2;
                    if padding > 0 {
                        sb.append("~");
                        padding -= 1;
                        sb.append(&" ".repeat(padding));
                    }

                    sb.append(&welcome);
                }
            } else {
                sb.append("~");
            }
        } else {
            let len = editor.rows[file_row as usize]
                .size
                .saturating_sub(editor.col_offset);
            if len > 0 {
                sb.append(&editor.rows[file_row as usize].chars[editor.col_offset as usize..]);
            } else {
                sb.append("");
            }
        }
        sb.append(CLEAR_LINE);
        if y < editor.screen_rows - 1 {
            sb.append("\r\n");
        }
    }
}

fn scroll_screen(editor: &mut EditorConfig) {
    if editor.cursor_y < editor.row_offset {
        editor.row_offset = editor.cursor_y;
    }
    if editor.cursor_y >= editor.row_offset + editor.screen_rows {
        editor.row_offset = editor.cursor_y - editor.screen_rows + 1;
    }
    if editor.cursor_x < editor.col_offset {
        editor.col_offset = editor.cursor_x;
    }
    if editor.cursor_x >= editor.col_offset + editor.screen_cols {
        editor.col_offset = editor.cursor_x - editor.screen_cols + 1;
    }
}

fn editor_refresh_screen(editor: &mut EditorConfig, sb: &mut ScreenBuffer) {
    scroll_screen(editor);
    toggle_cursor(sb, false);
    set_cursor_position(Some(sb), 1, 1);
    editor_draw_rows(sb, editor);
    set_cursor_position(
        Some(sb),
        (editor.cursor_y - editor.row_offset) + 1,
        (editor.cursor_x - editor.col_offset) + 1,
    );
    toggle_cursor(sb, true);

    // flush stdout
    match io::stdout().flush() {
        Ok(_) => (),
        Err(e) => die("flushing stdout after refresh", &Some(Box::new(e))),
    }
}

fn read_input() -> u8 {
    let mut buf = [0; 1];
    match io::stdin().read(&mut buf) {
        Ok(_) => (),
        Err(e) => die("reading input", &Some(Box::new(e))),
    }
    buf[0]
}

fn read_key() -> u32 {
    let c = read_input();
    if c == 27 {
        // escape sequence
        let c1 = read_input();
        let c2 = read_input();
        if c1 == b'[' {
            if c2 > b'0' && c2 <= b'9' {
                let c3 = read_input();
                if c3 == b'~' {
                    // function key
                    match c2 {
                        b'1' => return EditorKey::Home as u32,
                        b'3' => return EditorKey::Delete as u32,
                        b'4' => return EditorKey::End as u32,
                        b'5' => return EditorKey::PageUp as u32,
                        b'6' => return EditorKey::PageDown as u32,
                        b'7' => return EditorKey::Home as u32,
                        b'8' => return EditorKey::End as u32,
                        _ => (),
                    }
                }
            } else {
                // arrow keys
                match c2 {
                    b'A' => return EditorKey::Up as u32,
                    b'B' => return EditorKey::Down as u32,
                    b'C' => return EditorKey::Right as u32,
                    b'D' => return EditorKey::Left as u32,
                    b'H' => return EditorKey::Home as u32,
                    b'F' => return EditorKey::End as u32,
                    _ => (),
                }
            }
        } else if c1 == b'O' {
            match c2 {
                b'H' => return EditorKey::Home as u32,
                b'F' => return EditorKey::End as u32,
                _ => (),
            }
        }
    }
    c as u32
}

// May want to return the character in the future
fn process_keypress(editor: &mut EditorConfig) {
    let c = read_key();

    if is_cntrl(c) && c == ctrl_key('q') {
        clear_and_reset_cursor(None);
        disable_raw_mode(&editor.orig_termios);
        std::process::exit(0);
    }

    if c == 0 {
        return;
    }

    if let Ok(key) = EditorKey::try_from(c) {
        move_cursor(editor, key);
    }

    // TODO: create a logging method that puts output at the bottom of the screen
}

fn init_editor(orig_termios: Termios) -> EditorConfig {
    let (screen_cols, screen_rows) = get_window_size();
    EditorConfig {
        orig_termios,
        screen_rows,
        screen_cols,
        cursor_x: 0,
        cursor_y: 0,
        num_rows: 0,
        rows: Vec::new(),
        row_offset: 0,
        col_offset: 0,
    }
}

fn main() {
    let orig_termios = setup_terminal();
    let mut editor = init_editor(orig_termios);
    let mut sb = ScreenBuffer::new();
    let args: Vec<String> = env::args().collect();
    if args.len() >= 2 {
        editor_open(&mut editor, &args[1]);
    }

    loop {
        editor_refresh_screen(&mut editor, &mut sb);
        process_keypress(&mut editor);
        sb.flush();
    }
}
