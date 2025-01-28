use super::transfer::{DownloadTarget, TargetType};
use crate::AppData;
use actix_web::web::Data;
use anyhow::{bail, Context, Result};
use async_channel::{Receiver, Sender};
use colored::*;
use file_owner::PathExt;
use futures::StreamExt;
use log::{error, info};
use nix::unistd::Uid;
use std::{fs, path::Path};

#[derive(Clone)]
pub struct Worker {
    _id: usize,
    app_data: Data<AppData>,
    drx: Receiver<DownloadTargetMessage>,
}

impl Worker {
    pub fn start(id: usize, app_data: Data<AppData>, drx: Receiver<DownloadTargetMessage>) {
        let s = Self {
            _id: id,
            app_data,
            drx,
        };

        let _join_handle = actix_rt::spawn(async move { s.work().await });
    }
    async fn work(&self) -> Result<()> {
        loop {
            // Wait for a DownloadTarget
            let dtm = self.drx.recv().await?;

            // Download the target
            let done_status = match download_target(&self.app_data, &dtm.download_target).await {
                Ok(_) => DownloadDoneStatus::Success,
                Err(_) => DownloadDoneStatus::Failed,
            };
            dtm.tx.send(done_status).await?;
        }
    }
}

async fn download_target(app_data: &Data<AppData>, target: &DownloadTarget) -> Result<()> {
    match target.target_type {
        TargetType::Directory => {
            if !Path::new(&target.to).exists() {
                fs::create_dir(&target.to)?;
                if Uid::effective().is_root() {
                    target.to.clone().set_owner(app_data.config.uid)?;
                }
                info!("{}: directory created", &target);
            }
        }
        TargetType::File => {
            // Delete file if already exists
            if !Path::new(&target.to).exists() {
                // ensure the parent directory exists
                create_directory_from_filepath(&target.to)?;

                info!("{}: download {}", &target, "started".yellow());
                match fetch(target, app_data.config.uid).await {
                    Ok(_) => info!("{}: download {}", &target, "succeeded".green()),
                    Err(e) => {
                        error!("{}: download {}: {}", &target, "failed".red(), e);
                        bail!(e)
                    }
                };
            } else {
                info!("{}: already exists", &target);
            }
        }
    }
    Ok(())
}

async fn fetch(target: &DownloadTarget, uid: u32) -> Result<()> {
    let tmp_path = format!("{}.downloading", &target.to);
    let mut tmp_file = tokio::fs::File::create(&tmp_path).await?;

    let url = target.from.clone().context("No URL found")?;
    let mut byte_stream = reqwest::get(url).await?.bytes_stream();

    while let Some(item) = byte_stream.next().await {
        tokio::io::copy(&mut item?.as_ref(), &mut tmp_file).await?;
    }
    if Uid::effective().is_root() {
        tmp_path.clone().set_owner(uid)?;
    }

    fs::rename(&tmp_path, &target.to)?;

    Ok(())
}

fn create_directory_from_filepath(file_path: &str) -> std::io::Result<()> {
    // Convert the file path to a Path
    let path = Path::new(file_path);

    // Get the parent directory of the file
    if let Some(parent) = path.parent() {
        // Create the directory and all its parent components recursively
        fs::create_dir_all(parent)?;
    }

    Ok(())
}

#[derive(Debug, Clone)]
pub struct DownloadTargetMessage {
    pub download_target: DownloadTarget,
    pub tx: Sender<DownloadDoneStatus>,
}

#[derive(Debug, Clone)]
pub enum DownloadDoneStatus {
    Success,
    Failed,
}
