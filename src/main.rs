#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use tauri::command;
use reqwest::Client;
use serde_json::{json, Value};
use regex::Regex;
use quick_xml::Reader;
use quick_xml::events::Event;
use std::time::Duration;

// Function to decode HTML entities
fn decode_html_entities(text: &str) -> String {
    text.replace("&#39;", "'")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
}

#[command]
async fn fetch_transcript(video_url: String) -> Result<String, String> {
    let client = Client::builder().cookie_store(true).build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let response = client.get(&video_url)
        .header("User-Agent", "Mozilla/5.0")
        .send()
        .await
        .map_err(|e| format!("Failed to fetch video page: {}", e))?
        .text()
        .await
        .map_err(|e| format!("Failed to read response text: {}", e))?;

    let re = Regex::new(r#"(?s)ytInitialPlayerResponse\s*=\s*(\{.*?\});"#).unwrap();
    let json_string = re.captures(&response).ok_or("Failed to find the JSON data in the page.".to_string())?[1].to_string();

    let parsed_json: Value = serde_json::from_str(&json_string).map_err(|e| format!("Failed to parse JSON data: {}", e))?;

    if let Some(caption_tracks) = parsed_json.pointer("/captions/playerCaptionsTracklistRenderer/captionTracks") {
        if let Some(first_track) = caption_tracks.as_array().unwrap().first() {
            let transcript_url = first_track["baseUrl"].as_str().unwrap();
            let transcript_response = client.get(transcript_url)
                .send()
                .await
                .map_err(|e| format!("Failed to fetch transcript: {}", e))?
                .text()
                .await
                .map_err(|e| format!("Failed to read transcript response text: {}", e))?;

            let mut reader = Reader::from_str(&transcript_response);
            reader.trim_text(true);

            let mut buf = Vec::new();
            let mut transcript_text = String::new();

            loop {
                match reader.read_event(&mut buf) {
                    Ok(Event::Text(e)) => {
                        let text = e.unescape_and_decode(&reader).map_err(|e| format!("Failed to decode text: {}", e))?;
                        let decoded_text = decode_html_entities(&text);
                        transcript_text.push_str(&decoded_text);
                        transcript_text.push(' ');
                    },
                    Ok(Event::End(_)) | Ok(Event::Start(_)) => {},
                    Ok(Event::Eof) => break,
                    Err(e) => return Err(format!("Error reading transcript XML: {}", e)),
                    _ => (),
                }
                buf.clear();
            }

            let single_line_transcript = transcript_text.replace("\n", " ").replace("\r", " ");
            Ok(single_line_transcript.trim().to_string())
        } else {
            Err("No caption tracks found.".to_string())
        }
    } else {
        Err("No captions found for this video.".to_string())
    }
}

#[command]
async fn summarize_text(inputText: String) -> Result<String, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let api_url = "https://api-inference.huggingface.co/models/pszemraj/led-large-book-summary";
    let api_token = "";  // Replace with your actual API token

    let request_body = json!({
        "inputs": inputText,
        "parameters": {
            "min_length": 100,
            "max_length": 450,
            "length_penalty": 1.0,
            "num_beams": 4,
            "no_repeat_ngram_size": 3,
            "encoder_no_repeat_ngram_size": 3,
            "repetition_penalty": 3.5,
            "early_stopping": true
        }
    });

    let max_retries = 5;
    let mut attempts = 0;

    while attempts < max_retries {
        attempts += 1;
        println!("Attempt {}: Waiting for the response...", attempts);

        let response = client.post(api_url)
            .bearer_auth(api_token)
            .json(&request_body)
            .send()
            .await;

        match response {
            Ok(res) if res.status().is_success() => {
                let response_body: Value = res.json().await.map_err(|e| format!("Failed to parse response: {}", e))?;
                let summary = response_body[0]["summary_text"]
                    .as_str()
                    .unwrap_or("Failed to summarize text")
                    .to_string();
                return Ok(summary);
            },
            Ok(res) => {
                eprintln!("API request failed with status: {}", res.status());
            },
            Err(e) => {
                eprintln!("Failed to send request: {} (attempt {}/{})", e, attempts, max_retries);
            }
        }

        if attempts < max_retries {
            tokio::time::sleep(Duration::from_secs(exponential_backoff(attempts))).await;
        }
    }

    Err("Failed to summarize after multiple attempts.".to_string())
}

fn exponential_backoff(attempt: u32) -> u64 {
    2u64.pow(attempt)
}

#[tokio::main]
async fn main() {
    fix_path_env::fix().expect("Failed to fix $PATH environment variable");
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![fetch_transcript, summarize_text])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
