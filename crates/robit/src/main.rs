use clap::Parser;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use robit::cli::Cli;

#[tokio::main]
async fn main() {
    let _ = Cli::parse();

    // match std::env::current_dir() {
    //     Ok(path) => println!("当前工作目录: {}", path.display()),
    //     Err(e) => eprintln!("获取当前工作目录失败: {}", e),
    // }

    let llm = rig::providers::openai::Client::from_url("sk-XXX", "https://api.deepseek.com");
    let deepseek = llm.agent("deepseek-chat").build();

    let response = deepseek.prompt("你是谁？")
            .await
            .expect("Failed to invoke deepseek.");

    println!("DeepSeek: {}", response);
}
