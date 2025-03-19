use std::sync::Arc;

use candle::{
    utils::{cuda_is_available, has_mkl, metal_is_available},
    DType, Device, Tensor,
};
use candle_nn::VarBuilder;
use candle_transformers::models::qwen2::{Config, Model};
use hf_hub::{api::tokio::Api, Repo, RepoType};
use thiserror::Error;
use tokenizers::{PaddingParams, Tokenizer, TruncationDirection};
use tokio::{task::spawn_blocking, time::Instant};
use tracing::{debug, warn};

use crate::config::ModelConfig;

use super::EmbeddingError;

async fn build_model_and_tokenizer(
    device: Device,
    model_id: String,
    revision: String,
) -> Result<(Model, Tokenizer), EmbeddingError> {
    let start = Instant::now();
    let repo = Repo::with_revision(model_id, RepoType::Model, revision);
    let (config_filename, tokenizer_filename, weights_filename) = {
        let api = Api::new()?;
        let api = api.repo(repo);
        let config = api.get("config.json").await?;
        let tokenizer = api.get("tokenizer.json").await?;
        let weights = api.get("pytorch_model.bin").await?;
        (config, tokenizer, weights)
    };
    let config = tokio::fs::read_to_string(config_filename).await?;
    let config: Config = serde_json::from_str(&config)?;
    let mut tokenizer: Tokenizer = Tokenizer::from_file(tokenizer_filename)?;
    tokenizer.with_padding(Some(PaddingParams {
        strategy: PaddingStrategy::BatchLongest,
        direction: PaddingDirection::Right,
        pad_to_multiple_of: Some(8),
        // TODO: use values provided in model config
        pad_id: 0,
        pad_type_id: 0,
        pad_token: "<|endoftext|>".to_owned(),
    }));
    tokenizer.with_truncation(None)?;
    let dtype = if device.is_cuda() {
        DType::BF16
    } else {
        DType::F32
    };
    let vb = VarBuilder::from_pth(&weights_filename, dtype, &device)?;
    let model = Model::new(&config, vb)?;
    debug!(
        "loaded model and tokenizer in {} ms",
        start.elapsed().as_millis()
    );
    Ok((model, tokenizer))
}

fn device() -> Result<Device, EmbeddingError> {
    if cuda_is_available() {
        debug!("using CUDA");
        Ok(Device::new_cuda(0)?)
    } else if metal_is_available() {
        debug!("using metal");
        Ok(Device::new_metal(0)?)
    } else {
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            warn!("Running on CPU, to run on GPU(metal), use the `-metal` binary");
        }
        #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
        {
            warn!("Running on CPU, to run on GPU, use the `-cuda` binary");
        }
        if has_mkl() {
            debug!("using MKL");
        } else {
            debug!("using CPU");
        }
        Ok(Device::Cpu)
    }
}

#[derive(Clone)]
pub struct EmbeddingModel {
    device: Device,
    model: Arc<ModelForCausalLM>,
    model_config: ModelConfig,
    tokenizer: Tokenizer,
}

impl EmbeddingModel {
    pub async fn new(cfg: ModelConfig) -> Result<Self, EmbeddingError> {
        let device = device()?;
        let (model, tokenizer) =
            build_model_and_tokenizer(device.clone(), cfg.id.clone(), cfg.revision.clone()).await?;

        Ok(Self {
            device,
            model: Arc::new(model),
            model_config: cfg,
            tokenizer,
        })
    }

    pub async fn generate_embedding(&self, text: String) -> Result<Vec<f32>, EmbeddingError> {
        let start = Instant::now();
        let embedding = spawn_blocking(move || -> Result<Vec<f32>, EmbeddingError> {
            let encoding = self.tokenizer.encode(text, true)?;
            encoding.truncate(
                self.model_config.max_input_size,
                1,
                TruncationDirection::Right,
            );
            let tokens = Tensor::new(encoding.get_ids().to_vec(), &self.device)?.unsqueeze(0)?;
            let embedding = self.model.forward(&token_ids, 0)?;
            Ok(embedding.to_vec1::<f32>()?)
        })
        .await?;
        debug!("embedding generated in {} ms", start.elapsed().as_millis());
        embedding
    }
}
