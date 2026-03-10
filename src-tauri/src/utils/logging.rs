use std::fs::OpenOptions;
use std::io::Write;

pub fn log_debug(msg: &str) {
    for _ in 0..5 {
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open("wardian_debug.log")
        {
            let _ = writeln!(file, "{}", msg);
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}
