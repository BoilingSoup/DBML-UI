//! Inject a key event (XTEST FakeInput keycode) on the GIVEN display.
//! Args: <x11_keycode> [down|up|tap] — 37 = left ctrl, 32 = 'o'. The
//! default is a tap (press+release); down/up hold a modifier for chords.
use x11rb::connection::Connection as _;
use x11rb::protocol::xtest::ConnectionExt as _;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let kc = args[1].parse::<u8>()?;
    let mode = args.get(2).map(String::as_str).unwrap_or("tap");
    let (conn, _screen) = x11rb::connect(None)?;
    let (press, release) = (2u8, 3u8);
    let fire = |kind: u8| {
        conn.xtest_fake_input(kind, kc, 0, 0, 0, 0, 0).unwrap();
    };
    match mode {
        "down" => fire(press),
        "up" => fire(release),
        _ => {
            fire(press);
            conn.flush().unwrap();
            std::thread::sleep(std::time::Duration::from_millis(60));
            fire(release);
        }
    }
    conn.flush().unwrap();
    Ok(())
}
