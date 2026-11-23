use crate::{
    usage::Tokens,
    usage_prices::{self, Reason},
};

// Run before Tauri, credential access or gateway startup. Packaging verification
// must exercise the shipped executable, rather than another build's test binary.
pub fn run() -> Result<serde_json::Value, String> {
    let tokens = Tokens {
        input: 1000,
        cache_read: 500,
        output: 200,
        ..Tokens::default()
    };
    let glm =
        usage_prices::estimate("glm-5.3", tokens).map_err(|_| "安装包缺少 GLM-5.3 计价规则")?;
    if (glm.high_cny - 0.0146).abs() > 1e-10 || (glm.low_cny - 0.0146).abs() > 1e-10 {
        return Err("安装包 GLM-5.3 计价校验失败".into());
    }
    let mut gpt_62 = serde_json::Map::new();
    for (model, short_usd, long_usd) in [
        ("gpt-6.2-sol", 0.00405, 1.088004),
        ("gpt-6.2-astra", 0.0162, 4.352016),
    ] {
        let mut checks = serde_json::Map::new();
        for (name, usage, expected) in [
            ("short_usd", tokens, short_usd),
            ("long_usd", Tokens { input: 272_001, ..Tokens::default() }, long_usd),
        ] {
            let price = usage_prices::estimate(model, usage)
                .map_err(|_| format!("安装包缺少 {model} 计价规则"))?;
            let dollars = price.high_cny / usage_prices::USD_TO_CNY;
            if (dollars - expected).abs() > 1e-10 || price.low_cny != price.high_cny {
                return Err(format!("安装包 {model} {name} 计价校验失败"));
            }
            checks.insert(name.into(), dollars.into());
        }
        gpt_62.insert(model.into(), checks.into());
    }
    let qwen = usage_prices::estimate("qwen4-max", Tokens {
        cache_write: 100,
        cache_write_long: 50,
        ..tokens
    })
    .map_err(|_| "安装包缺少 Qwen4-Max 计价规则")?;
    if (qwen.high_cny - 0.0185).abs() > 1e-10 || (qwen.low_cny - 0.0185).abs() > 1e-10 {
        return Err("安装包 Qwen4-Max 计价校验失败".into());
    }
    for model in ["qwen4-max-preview", "qwen4.0-max-preview"] {
        if !matches!(usage_prices::estimate(model, tokens), Err(issue) if issue.reason == Reason::PlanOnly)
        {
            return Err(format!("安装包 {model} 套餐计费校验失败"));
        }
    }
    if !matches!(usage_prices::estimate("unreleased-package-check-model", tokens),
        Err(issue) if issue.reason == Reason::MissingPrice)
    {
        return Err("安装包未知模型计价校验失败".into());
    }
    Ok(serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "pricing_verified": usage_prices::VERIFIED,
        "pricing_contract": 2,
        "glm_5_3_cny": glm.high_cny,
        "gpt_6_2": gpt_62,
        "qwen4_max_cny": qwen.high_cny,
        "qwen4_aliases": "plan_only",
        "unknown_model": "missing_price"
    }))
}
