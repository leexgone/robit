use clap::Parser;
use rig::client::CompletionClient;
use rig::completion::Prompt;
use rig::providers::deepseek::ClientBuilder;
use robit::cli::Cli;
use robit::config::load_config;

#[tokio::main]
async fn main() {
    let _ = Cli::parse();

    // match std::env::current_dir() {
    //     Ok(path) => println!("当前工作目录: {}", path.display()),
    //     Err(e) => eprintln!("获取当前工作目录失败: {}", e),
    // }

    // let llm = rig::providers::openai::Client::from_url("sk-XXX", "https://api.deepseek.com");
    let config = load_config().unwrap();
    
    let llm = ClientBuilder::new(&config.llm().api_key).base_url(&config.llm().base_url).build().unwrap();
    let agent = llm.agent(&config.llm().model).build();

    let response = agent.prompt("你是谁？")
            .await
            .expect("Failed to invoke llm.");

    println!("{}", response);
}
