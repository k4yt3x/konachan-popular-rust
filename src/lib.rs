/*
 * Copyright (C) 2021-2023 K4YT3X.
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
use reqwest::Client;
use serde::Deserialize;
use serde_json;
use slog::{debug, error, info, warn};
use teloxide::{
    adaptors::throttle::{Limits, Throttle},
    prelude::*,
    types::{ChatId, InputFile, InputMedia, InputMediaPhoto},
};

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
    logger: slog::Logger,
    token: String,
    chat_id: ChatId,
}

impl Config {
    pub fn new(logger: slog::Logger, token: String, chat_id: i64) -> Config {
        Config {
            logger,
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
    for attempt in 0..NUMBER_OF_RETRIES {
        // send the photo with the caption
        info!(config.logger, "Sending posts: attempt={}", attempt);
        let result = bot
            .send_media_group(config.chat_id, posts.clone())
            .disable_notification(true)
            .await;

        match result {
            // if an error has occurred, print the error's message
            Err(error) => {
                warn!(
                    config.logger,
                    "Temporary error sending artwork: message={:?}", error
                );

                // if the last attempt still fails, return the error
                if attempt == NUMBER_OF_RETRIES - 1 {
                    return Err(error.into());
                }
            }
            // return if the send operation has succeeded
            Ok(messages) => {
                debug!(
                    config.logger,
                    "Successfully sent artwork: attempt={}", attempt
                );
                for message in messages {
                    debug!(config.logger, "API response: message={:#?}", message);
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
        config.logger,
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
    let today = Utc::now().format("%B %-d, %Y").to_string();
    info!(config.logger, "Fetching posts: date={}", today);

    // save all downloaded posts into memory
    let mut posts = Vec::new();
    for post in get_konachan_popular().await? {
        // if the original file's size is larger than 5 MiB
        // Telegram will not be able to download it
        let url = if post.file_size < TELEGRAM_MAX_DOWNLOAD_SIZE {
            post.jpeg_url
        }
        // if the sample image's size is within limits, use the sample image
        else if post.sample_file_size < TELEGRAM_MAX_DOWNLOAD_SIZE {
            post.sample_url
        }
        // skip if the sample image's size also exceeds the max allowable size
        else {
            continue;
        };

        posts.push(InputMedia::Photo(InputMediaPhoto::new(InputFile::url(
            url.parse()?,
        ))));
    }

    // send today's date
    bot.send_message(config.chat_id, today).await?;

    // send posts in groups of 10 images
    for group in posts.chunks(10) {
        if let Err(error) = send_posts(config.clone(), bot.clone(), group.to_vec()).await {
            error!(config.logger, "Failed sending posts: message={}", error);
        }
    }

    Ok(())
}
