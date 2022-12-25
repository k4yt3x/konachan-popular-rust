/*
 * Copyright (C) 2021-2022 K4YT3X.
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
use std::{
    io::{Cursor, Read},
    time::Duration,
};

use anyhow::Result;
use chrono::Utc;
use futures::future;
use image::{imageops::FilterType, ImageError, ImageFormat};
use reqwest::Client;
use serde::Deserialize;
use serde_json;
use slog::{debug, error, info, warn};
use teloxide::{
    prelude::*,
    types::{ChatId, InputFile, InputMedia, InputMediaPhoto},
    RequestError,
};
use teloxide_core::adaptors::throttle::{Limits, Throttle};
use tokio::{task, task::JoinHandle};

pub const VERSION: &'static str = env!("CARGO_PKG_VERSION");
const MAX_IMAGE_SIZE: usize = 10 * 1024_usize.pow(2);

#[derive(Clone, Debug, Deserialize)]
struct Post {
    id: u32,
    jpeg_url: String,
    jpeg_width: u32,
    jpeg_height: u32,
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

async fn get_konachan_popular() -> Result<Vec<Post>> {
    let posts: Vec<Post> = serde_json::from_str(
        &reqwest::get("https://konachan.com/post/popular_recent.json")
            .await?
            .text()
            .await?,
    )?;

    Ok(posts)
}

/// download an image into memory into Vec<u8>
///
/// # Arguments
///
/// * `url` - URL of the image
/// * `referer` - Referer header to set
///
/// # Errors
///
/// reqwest errors
///
/// # Examples
///
/// ```
/// let image_bytes = download_image(&"https://example.com/example.png",
/// &"https://example.com").await?
/// ```
async fn download_post(config: Config, post: Post) -> Result<Vec<u8>> {
    let original_image = reqwest::Client::new()
        .get(&post.jpeg_url)
        .send()
        .await?
        .bytes()
        .await?
        .to_vec();

    Ok(resize_image(
        &config,
        original_image,
        post.id,
        post.jpeg_width,
        post.jpeg_height,
    )
    .await?)
}

/// resize an image into a size/dimension acceptable by
/// Telegram's API
///
/// # Arguments
///
/// * `config` - an instance of Config
/// * `image_bytes` - raw input image bytes
/// * `id` - illustration ID
/// * `original_width` - original image width
/// * `original_height` - original image height
///
/// # Errors
///
/// image::ImageError
async fn resize_image(
    config: &Config,
    image_bytes: Vec<u8>,
    id: u32,
    original_width: u32,
    original_height: u32,
) -> Result<Vec<u8>, ImageError> {
    // if image is already small enough, return original image
    if image_bytes.len() <= MAX_IMAGE_SIZE {
        return Ok(image_bytes);
    }
    info!(config.logger, "Resizing oversized image: id={}", id);

    // this is a very rough guess
    // could be improved in the future
    let guessed_ratio = (MAX_IMAGE_SIZE as f32 / image_bytes.len() as f32).sqrt();
    let mut target_width = (original_width as f32 * guessed_ratio) as u32;
    let mut target_height = (original_height as f32 * guessed_ratio) as u32;
    debug!(
        config.logger,
        "Resizing parameters: r={} w={} h={}", guessed_ratio, target_width, target_height
    );

    // Telegram API requires width + height <= 10000
    if target_width + target_height > 10000 {
        let target_ratio = 10000.0 / (target_width + target_height) as f32;
        target_width = (target_width as f32 * target_ratio).floor() as u32;
        target_height = (target_height as f32 * target_ratio).floor() as u32;
        debug!(
            config.logger,
            "Additional resizing parameters: r={} w={} h={}",
            target_ratio,
            target_width,
            target_height
        );
    }

    // load the image from memory into ImageBuffer
    let mut dynamic_image = image::load_from_memory(&image_bytes)?;

    loop {
        // downsize the image with Lanczos3
        dynamic_image = dynamic_image.resize(target_width, target_height, FilterType::Lanczos3);

        // encode raw bytes into PNG bytes
        let mut png_bytes_cursor = Cursor::new(vec![]);
        dynamic_image.write_to(&mut png_bytes_cursor, ImageFormat::Png)?;

        // read all bytes from cursor
        let mut png_bytes = Vec::new();
        png_bytes_cursor.read_to_end(&mut png_bytes)?;

        // return the image if it is small enough
        if png_bytes.len() < MAX_IMAGE_SIZE {
            info!(
                config.logger,
                "Final size: size={}MiB",
                png_bytes.len() as f32 / 1024_f32.powf(2.0)
            );
            return Ok(png_bytes);
        }

        // shrink image by another 20% if the previous round is not enough
        debug!(
            config.logger,
            "Image too large: size={}MiB; additional resizing required",
            png_bytes.len() as f32 / 1024_f32.powf(2.0)
        );
        target_width = (target_width as f32 * 0.8) as u32;
        target_height = (target_height as f32 * 0.8) as u32;
    }
}

/// send an illustration to the Telegram chat
///
/// # Arguments
///
/// * `config` - an instance of Config
/// * `bot` - an instance of Throttle<Bot>
/// * `illust` - an Illust struct which represents an illustration
/// * `send_sleep` - global sleep timer
///
/// # Errors
///
/// any error that implements the Error trait
async fn send_posts<'a>(config: Config, bot: Throttle<Bot>, posts: &[Vec<u8>]) -> Result<()> {
    // holds all InputMedia enums for sendMediaGroup
    let mut images = Vec::new();

    // add each manga into images
    for post in posts {
        images.push(InputMedia::Photo(InputMediaPhoto {
            media: InputFile::memory(post.clone()),
            caption: None,
            parse_mode: None,
            caption_entities: None,
        }));
    }
    // contains the final result
    let mut result: Option<Result<Vec<Message>, RequestError>> = None;

    // retry up to 10 times since the send attempt might run into temporary errors like
    // Api(Unknown("Bad Request: group send failed"))
    for attempt in 0..10 {
        // send the photo with the caption
        info!(config.logger, "Sending posts: attempt={}", attempt);
        result = Some(
            bot.send_media_group(config.chat_id, images.clone())
                .disable_notification(true)
                .await,
        );

        // if an error has occurred, print the error's message
        if let Some(Err(error)) = &result {
            warn!(
                config.logger,
                "Temporary error sending artwork: message={:?}", error
            );
        }
        // break out of the loop if the send operation has succeeded
        else {
            debug!(
                config.logger,
                "Successfully sent artwork: attempt={}", attempt
            );
            break;
        }
    }

    // return the error if the send operation has not succeeded after 10 attempts
    if let Some(Err(error)) = result {
        Err(error.into())
    }
    else {
        Ok(())
    }
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

    // start downloading all posts
    let mut download_image_tasks: Vec<JoinHandle<Result<Vec<u8>>>> = vec![];
    for post in get_konachan_popular().await? {
        download_image_tasks.push(task::spawn(download_post(config.clone(), post.clone())));
    }

    // save all downloaded posts into memory
    let mut posts = Vec::new();
    for result in future::join_all(download_image_tasks).await {
        match result? {
            Err(error) => error!(config.logger, "Failed to download post: message={}", error),
            Ok(post) => posts.push(post),
        }
    }

    // send today's date
    bot.send_message(config.chat_id, today).await?;

    // send posts in groups of 10 images
    for group in posts.chunks(10) {
        if let Err(error) = send_posts(config.clone(), bot.clone(), group).await {
            error!(config.logger, "Failed sending posts: message={}", error);
        }
    }

    Ok(())
}
