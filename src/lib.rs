/*
 * Copyright (C) 2021-2024 K4YT3X.
 *
 * This program is free software; you can redistribute it and/or
 * modify it under the terms of the GNU General Public License
 * as published by the Free Software Foundation; only version 2
 * of the License.
 *
 * This program is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with this program. If not, see <https://www.gnu.org/licenses/>.
 */
use std::time::Duration;

use anyhow::{anyhow, Result};
use chrono::Utc;
use regex::Regex;
use reqwest::Client;
use serde::Deserialize;
use teloxide::{
    adaptors::throttle::{Limits, Throttle},
    prelude::*,
    types::{InputFile, InputMedia, InputMediaPhoto},
};
use tracing::{debug, error, info, warn};

pub const VERSION: &'static str = env!("CARGO_PKG_VERSION");
const NUMBER_OF_RETRIES: u8 = 5;
const TELEGRAM_MAX_DOWNLOAD_SIZE: u64 = 5 * 1024_u64.pow(2);

#[derive(Clone, Debug, Deserialize)]
struct Post {
    file_size: u64,
    jpeg_url: String,
    sample_file_size: u64,
    sample_url: String,
}

/// configs passed to the run function
#[derive(Clone)]
pub struct Config {
    token: String,
    chat_id: ChatId,
}

impl Config {
    pub fn new(token: String, chat_id: i64) -> Config {
        Config {
            token,
            chat_id: ChatId(chat_id),
        }
    }
}

/// retrieve the list of popular Konachan posts
///
/// # Errors
///
/// anyhow::Error
async fn get_konachan_popular() -> Result<Vec<Post>> {
    let posts: Vec<Post> = serde_json::from_str(
        &reqwest::get("https://konachan.com/post/popular_recent.json")
            .await?
            .text()
            .await?,
    )?;

    Ok(posts)
}

/// send an illustration to the Telegram chat
///
/// # Arguments
///
/// * `config` - an instance of Config
/// * `bot` - an instance of Throttle<Bot>
/// * `posts` - a Vec of posts to send in one media group
///
/// # Errors
///
/// any error that implements the Error trait
async fn send_posts(config: Config, bot: Throttle<Bot>, posts: Vec<InputMedia>) -> Result<()> {
    // retry up to 5 times since the send attempt might run into temporary errors like
    // Api(Unknown("Bad Request: group send failed"))
    let mut posts_local = posts.clone();
    for attempt in 0..NUMBER_OF_RETRIES {
        // send the photo with the caption
        info!("Sending posts: attempt={}", attempt);
        let result = bot
            .send_media_group(config.chat_id, posts_local.clone())
            .disable_notification(true)
            .await;

        match result {
            // if an error has occurred, print the error's message
            Err(error) => {
                warn!("Temporary error sending artwork: message={:?}", error);

                let error_index_regex = Regex::new(r"#(\d+)")?;
                if let Some(captures) = error_index_regex.captures(error.to_string().as_str()) {
                    let index = captures.get(1).unwrap().as_str().parse::<usize>()? - 1;
                    warn!("Removing failed post: index={}", index);
                    posts_local.remove(index);
                }

                // if the last attempt still fails, return the error
                if attempt >= NUMBER_OF_RETRIES - 1 {
                    return Err(error.into());
                }

                // sleep for one second
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            // return if the send operation has succeeded
            Ok(messages) => {
                for message in messages {
                    debug!("API response: message={:#?}", message);
                }
                return Ok(());
            }
        }
    }

    Err(anyhow!("Sending attempt iteration error"))
}

/// entry point for the functional part of this program
///
/// # Arguments
///
/// * `config` - an instance of Config
///
/// # Errors
///
/// any error that implements the Error trait
pub async fn run(config: Config) -> Result<()> {
    info!(
        "KonachanPopular bot {version} initializing",
        version = VERSION
    );

    // initialize bot instance with a custom client
    // the default pool idle timeout is 90 seconds, which is too small for large
    // images to be uploaded
    let client = Client::builder()
        .pool_idle_timeout(Duration::from_secs(6000))
        .build()?;
    let bot = Bot::with_client(&config.token, client).throttle(Limits::default());

    // log today's date in the console
    let now = Utc::now();
    let today = now.format("%B %-d, %Y").to_string();
    info!("Fetching posts: date={}", today);

    // fetch the list of popular Konachan posts
    let mut popular_posts: Vec<Post> = Vec::new();
    for attempt in 0..NUMBER_OF_RETRIES {
        match get_konachan_popular().await {
            Ok(popular) => {
                popular_posts = popular;
                break;
            }
            Err(error) => {
                warn!(
                    "Failed to fetch today's popular posts: attempt={}: message={}",
                    attempt, error
                );

                // if the last attempt still fails, return the error
                if attempt >= NUMBER_OF_RETRIES - 1 {
                    return Err(error);
                }

                // sleep for one second
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    }

    // save all downloaded posts into memory
    let mut posts = Vec::new();
    for post in &popular_posts {
        // if the original file's size is larger than 5 MiB
        // Telegram will not be able to download it
        let url = if post.file_size < TELEGRAM_MAX_DOWNLOAD_SIZE {
            post.jpeg_url.clone()
        }
        // if the sample image's size is within limits, use the sample image
        else if post.sample_file_size < TELEGRAM_MAX_DOWNLOAD_SIZE {
            post.sample_url.clone()
        }
        // skip if the sample image's size also exceeds the max allowable size
        else {
            continue;
        };

        posts.push(InputMedia::Photo(InputMediaPhoto::new(InputFile::url(
            url.parse()?,
        ))));
    }

    // get image links
    let image_links = popular_posts
        .iter()
        .map(|post| post.jpeg_url.clone())
        .collect::<Vec<String>>()
        .join("\n");

    // send today's date
    bot.send_document(
        config.chat_id,
        InputFile::memory(image_links)
            .file_name(format!("{}_links.txt", now.format("%Y-%m-%d").to_string())),
    )
    .caption(&today)
    .await?;

    // send posts in groups of 10 images
    for (batch, group) in posts.chunks(10).enumerate() {
        info!("Sending posts: batch={}", batch);
        if let Err(error) = send_posts(config.clone(), bot.clone(), group.to_vec()).await {
            error!("Failed to send posts: batch={} message={}", batch, error);
        }
        else {
            info!("Successfully sent posts: batch={}", batch);
        }
    }

    Ok(())
}
