use anyhow::Result;
use tracing::debug;

use crate::nix::eval_nix_expr;

// Data structure for package metadata
#[derive(Debug)]
pub struct PackageMetadata {
    pub version: String,
    pub src_url: Option<String>,
    pub output_hash: Option<String>,
    pub cargo_hash: Option<String>,
    pub vendor_hash: Option<String>,
    pub pname: Option<String>,
}

pub struct PackageQuery {
    eval_entry_point: String,
    attr_path: String,
}

impl PackageQuery {
    pub fn new(eval_entry_point: &str, attr_path: &str) -> Self {
        // Normalize the entry point to a valid Nix filepath
        let eval_path = if eval_entry_point.starts_with('/') || eval_entry_point.starts_with('.') {
            eval_entry_point.to_string()
        } else {
            format!("./{}", eval_entry_point)
        };

        Self {
            eval_entry_point: eval_path,
            attr_path: attr_path.to_string(),
        }
    }

    pub async fn get_attr(&self, attr: &str) -> Option<String> {
        let expr = format!(
            "with import {} {{ }}; {}.{}",
            self.eval_entry_point, self.attr_path, attr
        );

        eval_nix_expr(&expr).await.ok()
    }

    pub async fn get_version(&self) -> Result<String> {
        // Try to get version directly
        let expr = format!(
            "with import {} {{ }}; {}.version or (builtins.parseDrvName {}.name).version",
            self.eval_entry_point, self.attr_path, self.attr_path
        );

        let res = eval_nix_expr(&expr).await?;
        Ok(res)
    }

    pub async fn get_src_url(&self) -> Option<String> {
        // Try to get source URL
        let url_expr = format!(
            "with import {} {{ }}; builtins.toString ({}.src.url or {}.src.urls)",
            self.eval_entry_point, self.attr_path, self.attr_path
        );

        eval_nix_expr(&url_expr).await.ok()
    }
}

impl PackageMetadata {
    /// Extract package metadata from Nix evaluation
    pub async fn from_attr_path(eval_entry_point: &str, attr_path: &str) -> anyhow::Result<Self> {
        debug!("Extracting metadata for {}", attr_path);
        let package = PackageQuery::new(eval_entry_point, attr_path);

        let version = package.get_version().await?;
        let src_url = package.get_src_url().await;
        let output_hash = package.get_attr("src.outputHash").await;
        let cargo_hash = package.get_attr("cargoHash").await;
        let vendor_hash = package.get_attr("vendorHash").await;
        let pname = package.get_attr("pname").await;

        Ok(PackageMetadata {
            version,
            src_url,
            output_hash,
            cargo_hash,
            vendor_hash,
            pname,
        })
    }
}
