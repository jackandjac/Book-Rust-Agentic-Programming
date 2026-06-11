// Chapter 4: Tool Calling with Rig
// See chapters/ch04-tool-calling.md for the full explanation.
//
// Run: cargo run -p ch04-tool-calling
// Requires: OPENAI_API_KEY env var (or .env file)

use anyhow::Result;
use rig::client::{CompletionClient, ProviderClient};
use rig::completion::Prompt;
use rig::providers::openai;
use rig::tool::ToolError;
use rig_derive::rig_tool;

#[rig_tool(
    description = "Get the current weather for a named city",
    params(city = "The city name, e.g. 'London' or 'New York'"),
    required(city)
)]
fn get_weather(city: String) -> Result<String, ToolError> {
    // Stub: replace with a real weather API call
    // e.g. OpenWeatherMap: https://api.openweathermap.org/data/2.5/weather?q={city}
    match city.to_lowercase().as_str() {
        "london" => Ok("London: 12°C, overcast".to_string()),
        "paris" => Ok("Paris: 18°C, sunny".to_string()),
        "tokyo" => Ok("Tokyo: 22°C, humid".to_string()),
        other => Ok(format!("{other}: 20°C, clear skies")),
    }
}

#[rig_tool(
    description = "Convert temperature between Celsius (C), Fahrenheit (F), and Kelvin (K)",
    params(
        value = "The numeric temperature to convert",
        from  = "Source unit: C, F, or K",
        to    = "Target unit: C, F, or K"
    ),
    required(value, from, to)
)]
fn convert_temperature(value: f64, from: String, to: String) -> Result<String, ToolError> {
    let celsius = match from.to_uppercase().as_str() {
        "C" => value,
        "F" => (value - 32.0) * 5.0 / 9.0,
        "K" => value - 273.15,
        other => {
            return Err(ToolError::ToolCallError(
                format!("Unknown source unit '{other}'. Use C, F, or K.").into()
            ))
        }
    };

    let result = match to.to_uppercase().as_str() {
        "C" => celsius,
        "F" => celsius * 9.0 / 5.0 + 32.0,
        "K" => celsius + 273.15,
        other => {
            return Err(ToolError::ToolCallError(
                format!("Unknown target unit '{other}'. Use C, F, or K.").into()
            ))
        }
    };

    Ok(format!("{value}°{from} = {result:.1}°{to}"))
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let agent = openai::Client::from_env()
        .agent(openai::GPT_4O)
        .preamble(
            "You are a helpful assistant with weather data and a temperature converter. \
             Use the tools when the user asks about weather or temperatures. Be concise.",
        )
        .tool(get_weather)
        .tool(convert_temperature)
        .build();

    let questions = [
        "What is the weather in London?",
        "What's the weather in Tokyo, and what is that in Fahrenheit?",
        "Convert 100°C to Kelvin.",
    ];

    for question in &questions {
        println!("\n> {question}");
        let response = agent.prompt(question).await?;
        println!("{response}");
    }

    Ok(())
}
