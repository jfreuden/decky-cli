use anyhow::{anyhow, Context, Result};
use boolinator::Boolinator;
use log::info;
use std::{
    fs::File,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use walkdir::WalkDir;
use zip::{write::FileOptions, ZipWriter};

use crate::{
    docker,
    plugin::{CustomBackend, Plugin},
};

pub struct Builder {
    docker_image: String,

    pub plugin: Plugin,
    pub plugin_root: PathBuf,
    pub output_root: PathBuf,
    pub tmp_build_root: PathBuf,
}

impl Builder {
    pub async fn build_frontend(&self) -> Result<()> {
        info!("Building frontend");

        docker::run_image(
            self.docker_image.clone(),
            vec![
                (
                    self.plugin_root.canonicalize()?.to_str().unwrap().into(),
                    "/plugin".into(),
                ),
                (self.tmp_build_root.to_str().unwrap().into(), "/out".into()),
            ],
        )
        .await
    }

    pub async fn build_backend(&self) -> Result<()> {
        if !&self.plugin_root.join("backend").exists() {
            info!("Plugin does not have a custom backend");
            return Ok(());
        }

        info!("Building backend");
        let mut image_tag: String = self.docker_image.clone();

        match self.plugin.custom_backend {
            CustomBackend::Dockerfile => {
                image_tag = docker::build_image(
                    self.plugin_root.join("backend").join("Dockerfile"),
                    self.plugin.meta.name.to_lowercase().clone(),
                )
                .await?
                .clone();
            }
            CustomBackend::None => {}
        }

        docker::run_image(
            image_tag.into(),
            vec![
                (
                    self.plugin_root
                        .join("backend")
                        .canonicalize()?
                        .to_str()
                        .unwrap()
                        .into(),
                    "/backend".into(),
                ),
                (
                    self.tmp_build_root.join("bin").to_str().unwrap().into(),
                    "/backend/out".into(),
                ),
            ],
        )
        .await
    }

    fn zip_path(&self, path: PathBuf, zip: &mut ZipWriter<File>) -> Result<()> {
        let mut buffer = Vec::new();

        let name = path
            .strip_prefix(&self.tmp_build_root)
            .map(|name| name.to_path_buf())
            .and_then(|name| {
                name.strip_prefix("defaults")
                    .map(|path| path.to_path_buf())
                    .or(Ok(name))
            })
            .map(|name| Path::new(&self.plugin.meta.name).join(name))?;

        if path.is_file() {
            let mut f = std::fs::File::open(&path)?;
            f.read_to_end(&mut buffer)?;

            zip.start_file(name.to_str().unwrap(), FileOptions::default())?;

            zip.write(&*buffer)?;
            buffer.clear();
        } else if !name.as_os_str().is_empty() {
            zip.add_directory(name.to_str().unwrap(), FileOptions::default())?;
        }

        Ok(())
    }

    pub fn zip_plugin(&self) -> Result<()> {
        info!("Zipping plugin");
        let filename = format!("{}.zip", &self.plugin.meta.name);
        let file = std::fs::File::create(&self.output_root.join(filename))
            .expect("Could not create zip file");
        let mut zip = zip::ZipWriter::new(file);

        let directories = vec![("dist", true), ("bin", false), ("defaults", false)];
        let files = vec![
            "LICENSE",
            "main.py",
            "package.json",
            "plugin.json",
            "README.md",
        ];

        for file in files {
            let full_path = self.tmp_build_root.join(&file);
            self.zip_path(full_path, &mut zip)?;
        }

        for directory in directories {
            let full_path = self.tmp_build_root.join(&directory.0);

            if directory.1 == false && !full_path.exists() {
                info!("Optional directory {} not found. Continuing", &directory.0);
                continue;
            }

            let dir_entries = WalkDir::new(full_path);

            for entry in dir_entries {
                let file = entry?;
                self.zip_path(file.path().to_path_buf(), &mut zip)?;
            }
        }

        zip.finish()?;

        Ok(())
    }

    fn validate_tmp_output_root(tmp_output_root: &PathBuf) -> Result<&PathBuf> {
        Ok(tmp_output_root).and_then(|path| {
            path.is_absolute().as_result(
                path,
                anyhow!("For safety reasons, tmp_output_root must be an absolute path"),
            )
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        info!("Creating temporary build directory");
        std::fs::remove_dir_all(&self.tmp_build_root).ok();
        std::fs::create_dir_all(&self.tmp_build_root)
            .context("Temporary build directory already exists")?;

        info!("Building plugin");
        self.build_backend().await?;
        self.build_frontend().await?;
        self.zip_plugin()?;

        Ok(())
    }

    pub fn new(
        plugin_root: PathBuf,
        output_root: PathBuf,
        tmp_output_root: PathBuf,
    ) -> Result<Self> {
        if !output_root.exists() {
            std::fs::create_dir(&output_root)?;
        }

        Builder::validate_tmp_output_root(&tmp_output_root).unwrap();

        Ok(Self {
            plugin: Plugin::new(plugin_root.clone()).expect("Could not create plugin"),
            plugin_root: plugin_root
                .canonicalize()
                .expect("Could not find plugin root"),
            output_root: output_root
                .canonicalize()
                .expect("Could not find output root"),
            tmp_build_root: tmp_output_root,
            docker_image: "ghcr.io/steamdeckhomebrew/builder:latest".to_owned(),
        })
    }
}
