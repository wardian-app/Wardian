use std::process::ExitCode;

fn main() -> ExitCode {
    let arguments = std::env::args().skip(1).collect::<Vec<_>>();
    match wardian_app_lib::run_chat_cache_admin(&arguments) {
        Ok((report, complete)) => {
            match serde_json::to_string_pretty(&report) {
                Ok(report) => println!("{report}"),
                Err(error) => {
                    eprintln!("cannot encode maintenance report: {error}");
                    return ExitCode::FAILURE;
                }
            }
            if complete {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(2)
            }
        }
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(64)
        }
    }
}
