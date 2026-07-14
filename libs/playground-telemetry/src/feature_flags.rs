use open_feature::EvaluationContext;
use open_feature::provider::FeatureProvider;
use open_feature_flagd::{FlagdOptions, FlagdProvider, ResolverType};
use tokio::sync::OnceCell;

static FLAGD_PROVIDER: OnceCell<FlagdProvider> = OnceCell::const_new();

/// Evaluates a flagd boolean flag with an explicit environment escape hatch.
///
/// The environment value is deliberately additive: labs stay reproducible when
/// flagd is unavailable, while a configured flagd can enable the same path for
/// every service. Each evaluation is logged with OpenFeature-aligned fields.
pub async fn feature_flag(flag_key: &'static str, env_name: &'static str) -> bool {
    let env_override = env_flag(env_name);
    let mut provider_name = "flagd";
    let mut provider_value = false;
    let mut variant = "off".to_string();
    let mut error = String::new();

    match flagd_provider().await {
        Ok(provider) => match provider
            .resolve_bool_value(flag_key, &EvaluationContext::default())
            .await
        {
            Ok(details) => {
                provider_value = details.value;
                variant = details
                    .variant
                    .unwrap_or_else(|| if provider_value { "on" } else { "off" }.to_string());
            }
            Err(err) => {
                provider_name = "env";
                error = format!("{err:?}");
            }
        },
        Err(err) => {
            provider_name = "env";
            error = err.to_string();
        }
    }

    let effective = provider_value || env_override;
    if env_override {
        variant = "env-on".to_string();
    }
    tracing::info!(
        "feature_flag.key" = flag_key,
        "feature_flag.provider_name" = provider_name,
        "feature_flag.variant" = %variant,
        "feature_flag.value" = effective,
        "feature_flag.env_override" = env_override,
        "feature_flag.error" = %error,
        "feature_flag.evaluation"
    );
    effective
}

fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
}

async fn flagd_provider() -> anyhow::Result<&'static FlagdProvider> {
    FLAGD_PROVIDER
        .get_or_try_init(|| async {
            FlagdProvider::new(FlagdOptions {
                resolver_type: ResolverType::Rpc,
                ..Default::default()
            })
            .await
            .map_err(anyhow::Error::new)
        })
        .await
}
