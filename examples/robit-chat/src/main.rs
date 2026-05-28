//! robit-chat: REPL interactive chat for Phase 1 validation.
//!
//! Usage: cargo run -p robit-chat

use futures::StreamExt;
use robit_ai::config::{load_env, load_llm_config, load_settings};
use robit_ai::{
    ChatCompletionRequestAssistantMessage, ChatCompletionRequestMessage,
    ChatCompletionRequestSystemMessage, ChatCompletionRequestUserMessage, LlmClient,
};
use async_openai::types::ChatCompletionRequestAssistantMessageContent;
use std::io::{self, BufRead, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Load environment variables from ~/.robit/.env
    load_env();

    // Load configuration
    let llm_config = load_llm_config()?;
    let settings = load_settings()?;

    // Create LLM client
    let client = LlmClient::from_config(&llm_config, &settings)?;
    println!(
        "Robit Chat | provider: {} | model: {}",
        client.provider(),
        client.model()
    );
    println!("输入消息开始对话，输入 exit 或 Ctrl+D 退出\n");

    // Conversation history
    let mut messages: Vec<ChatCompletionRequestMessage> = vec![
        ChatCompletionRequestMessage::System(
            ChatCompletionRequestSystemMessage {
                content: "你是 Robit，一个 AI 编程代理。请直接回答用户问题。".into(),
                name: None,
            }
            .into(),
        ),
    ];

    let stdin = io::stdin();
    let mut stdout = io::stdout();

    loop {
        // Read user input
        print!("> ");
        stdout.flush()?;

        let mut input = String::new();
        if stdin.lock().read_line(&mut input)? == 0 {
            // EOF (Ctrl+D)
            break;
        }

        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input == "exit" || input == "/exit" {
            break;
        }

        // Append user message to history
        messages.push(ChatCompletionRequestMessage::User(
            ChatCompletionRequestUserMessage {
                content: input.to_string().into(),
                name: None,
            }
            .into(),
        ));

        // Stream response
        print!("Robit: ");
        stdout.flush()?;

        let mut stream = client.chat_stream(messages.clone(), None).await?;
        let mut full_response = String::new();

        while let Some(chunk_result) = stream.next().await {
            match chunk_result {
                Ok(chunk) => {
                    if let Some(choice) = chunk.choices.first() {
                        if let Some(content) = &choice.delta.content {
                            print!("{}", content);
                            stdout.flush()?;
                            full_response.push_str(content);
                        }
                    }
                }
                Err(e) => {
                    eprintln!("\n[错误] {}", e);
                    break;
                }
            }
        }

        println!(); // newline after response

        // Append assistant response to history
        if !full_response.is_empty() {
            messages.push(ChatCompletionRequestMessage::Assistant(
                ChatCompletionRequestAssistantMessage {
                    content: Some(ChatCompletionRequestAssistantMessageContent::Text(
                        full_response,
                    )),
                    name: None,
                    tool_calls: None,
                    refusal: None,
                    audio: None,
                    #[allow(deprecated)]
                    function_call: None,
                }
                .into(),
            ));
        }
    }

    println!("\n再见！");
    Ok(())
}
