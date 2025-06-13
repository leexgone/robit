use clap::Parser;
use robit::cli::Cli;

fn main() {
    let _ = Cli::parse();

    match std::env::current_dir() {
        Ok(path) => println!("当前工作目录: {}", path.display()),
        Err(e) => eprintln!("获取当前工作目录失败: {}", e),
    }

    println!();
    println!("{}", robit::config::LICENSE);
}
