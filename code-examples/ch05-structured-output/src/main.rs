// Chapter 5: Structured Output with Rig
// See chapters/ch05-structured-output.md for the full explanation.
//
// Run: cargo run -p ch05-structured-output
// Requires: OPENAI_API_KEY env var (or .env file)

use anyhow::Result;
use rig::client::ProviderClient;
use rig::providers::openai;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct WorkExperience {
    /// Employer name
    company: String,
    /// Role or job title
    title: String,
    /// Year started
    start_year: Option<u32>,
    /// Year ended — null if current role
    #[schemars(required)]
    end_year: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema, Serialize)]
struct Resume {
    /// Candidate's full name — null if not found
    #[schemars(required)]
    name: Option<String>,
    /// Email address — null if not mentioned
    #[schemars(required)]
    email: Option<String>,
    /// Technical and professional skills listed
    skills: Vec<String>,
    /// Work history, most recent first
    experience: Vec<WorkExperience>,
    /// Highest education level mentioned — null if not mentioned
    #[schemars(required)]
    education: Option<String>,
}

const RESUME_TEXT: &str = r#"
Sarah Chen | sarah.chen@example.com

Experienced software engineer with 8 years in distributed systems.

Skills: Rust, Go, Kubernetes, PostgreSQL, Redis, Apache Kafka

Experience:
- Staff Engineer, DataFlow Inc. (2022–present)
  Leading distributed ingestion pipeline team
- Senior Software Engineer, CloudBase (2019–2022)
  Built autoscaling microservices in Go

Education: B.S. Computer Science, UC Berkeley (2015)
"#;

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let extractor = openai::Client::from_env()?
        .extractor::<Resume>(openai::GPT_4O_MINI)
        .preamble(
            "Extract structured resume data from the provided text. \
             Use null for any field that is not mentioned. \
             For experience, extract all positions listed.",
        )
        .retries(2)
        .build();

    println!("Parsing resume...\n");
    let resume = extractor.extract(RESUME_TEXT).await?;

    println!("Name:      {:?}", resume.name);
    println!("Email:     {:?}", resume.email);
    println!("Education: {:?}", resume.education);
    println!("\nSkills ({}):", resume.skills.len());
    for skill in &resume.skills {
        println!("  - {skill}");
    }
    println!("\nExperience ({} positions):", resume.experience.len());
    for job in &resume.experience {
        println!(
            "  {} at {} ({:?}–{:?})",
            job.title, job.company, job.start_year, job.end_year
        );
    }

    Ok(())
}
