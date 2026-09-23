//! 藏宝阁公开商品读取。
//!
//! 该模块只读取用户主动粘贴的藏宝阁商品链接，并把公开返回的御魂与式神库存
//! 转换为现有导入链路能够校验的快照；不读取登录态，也不尝试访问交易或账户接口。

use crate::application::{
    error::AppError,
    importer::{is_desktop_special_hero_id, ImportFileInput},
    security::{ensure_bytes, MAX_IMPORT_FILE_BYTES, MAX_IMPORT_SHIKIGAMI, MAX_IMPORT_SOULS},
};
use crate::domain::{PLATFORM_ANDROID, PLATFORM_IOS};
use chrono::Utc;
use reqwest::{blocking::Client, Url};
use serde::Serialize;
use serde_json::{json, Value};
use std::time::Duration;

/// 藏宝阁公开详情接口；商品链接只用于提取 serverid 和 ordersn，不接受任意远程地址。
const CBG_DETAIL_API: &str = "https://yys.cbg.163.com/cgi/api/get_equip_detail";
/// 公开接口单次返回大小上限沿用普通 JSON 导入预算，避免异常响应占满内存。
const MAX_CBG_RESPONSE_BYTES: usize = MAX_IMPORT_FILE_BYTES;
/// 预览最多返回 5 枚御魂，界面只需要证明字段形状，不应把整份库存搬进 WebView。
const CBG_PREVIEW_SAMPLE_SIZE: usize = 5;

/// 藏宝阁预检结果；只暴露用户确认导入所需的统计和少量样例，不返回原始账户正文。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CbgReadPreview {
    pub source_url: String,
    pub account_name: Option<String>,
    pub server_name: Option<String>,
    /// 只有藏宝阁公开详情明确返回安卓/iOS标识时才填写，无法确认时保持未知。
    pub platform: Option<String>,
    pub inventory_count: u32,
    pub retained_count: u32,
    pub six_star_count: u32,
    pub lower_star_count: u32,
    pub shikigami_count: u32,
    pub sample_souls: Vec<CbgSoulPreview>,
    pub warnings: Vec<String>,
    pub message: String,
}

/// 藏宝阁样例御魂；字段名称保持接近用户在商品页看到的内容，便于确认映射是否正确。
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CbgSoulPreview {
    pub id: String,
    pub set_name: String,
    pub slot: u8,
    pub quality: u8,
    pub level: u8,
    pub main_attribute: String,
    pub main_value: String,
    pub sub_attributes: Vec<String>,
}

/// 预检和正式导入共用的准备结果，避免两条路径各自维护一套藏宝阁字段转换。
pub struct CbgImportPreparation {
    pub file: ImportFileInput,
    pub preview: CbgReadPreview,
}

/// 执行一次藏宝阁公开详情读取并生成标准快照载荷。
pub struct CbgReadUseCase;

impl CbgReadUseCase {
    /// 读取链接并只返回预览统计；不会写入数据库或当前库存。
    pub fn inspect(source_url: &str) -> Result<CbgReadPreview, AppError> {
        Ok(Self::prepare(source_url)?.preview)
    }

    /// 读取链接、转换御魂字段并生成现有导入用例可消费的 JSON；此处仍不提交当前库存。
    pub fn prepare(source_url: &str) -> Result<CbgImportPreparation, AppError> {
        let source_url = source_url.trim().to_owned();
        let (server_id, order_sn) = parse_cbg_url(&source_url)?;
        let response = fetch_detail(&source_url, &server_id, &order_sn)?;
        let equip = response
            .get("equip")
            .and_then(Value::as_object)
            .ok_or_else(|| AppError::invalid_argument("sourceUrl", "藏宝阁返回中缺少商品详情"))?;
        let equip_desc = equip
            .get("equip_desc")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                AppError::invalid_argument("sourceUrl", "藏宝阁商品未公开账号御魂库存")
            })?;
        let equip_desc: Value = serde_json::from_str(equip_desc).map_err(|error| {
            AppError::invalid_argument(
                "sourceUrl",
                format!("藏宝阁商品详情中的御魂库存数据不是有效 JSON：{error}"),
            )
        })?;
        let inventory = equip_desc
            .get("inventory")
            .and_then(Value::as_object)
            .ok_or_else(|| {
                AppError::invalid_argument("sourceUrl", "该藏宝阁商品没有公开御魂库存")
            })?;
        if inventory.len() > MAX_IMPORT_SOULS {
            return Err(AppError::import_field(
                "藏宝阁公开御魂库存",
                "$.inventory",
                &format!("不超过 {MAX_IMPORT_SOULS} 枚"),
                &inventory.len().to_string(),
                "藏宝阁公开御魂数量超过安全上限",
            ));
        }

        // heroes 与 inventory 都来自同一份公开快照；缺少 heroes 时保留御魂导入能力，并按 0 个式神处理。
        let heroes = equip_desc
            .get("heroes")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let heroes_object = heroes.as_object().ok_or_else(|| {
            AppError::invalid_argument("sourceUrl", "藏宝阁商品中的式神数据格式不正确")
        })?;
        if heroes_object.len() > MAX_IMPORT_SHIKIGAMI {
            return Err(AppError::import_field(
                "藏宝阁公开式神数据",
                "$.heroes",
                &format!("不超过 {MAX_IMPORT_SHIKIGAMI} 个"),
                &heroes_object.len().to_string(),
                "藏宝阁公开式神数量超过安全上限",
            ));
        }
        let shikigami_count = count_importable_cbg_heroes(heroes_object);

        let mut souls = Vec::with_capacity(inventory.len());
        for (fallback_id, raw_soul) in inventory {
            souls.push(parse_cbg_soul(fallback_id, raw_soul)?);
        }

        let inventory_count = souls.len() as u32;
        let six_star_count = souls.iter().filter(|soul| soul.quality == 6).count() as u32;
        let lower_star_count = inventory_count.saturating_sub(six_star_count);
        let account_name = optional_text(equip, "equip_name");
        let server_name = optional_text(equip, "server_name");
        // 平台优先从公开详情字段读取；字段缺失时回退到同一商品页主卡片图标，避免把推荐商品图标误当成当前商品。
        let platform = detect_cbg_platform(equip, &equip_desc)
            .or_else(|| fetch_cbg_page_platform(&source_url));
        let mut warnings = vec![
            "藏宝阁公开数据按完整御魂与式神快照导入；商品下架或详情变化后需重新读取。".to_owned(),
            "公开 heroes 会写入“式神录”；式神装备关系、货币和商品价格等字段不会写入本地库存。"
                .to_owned(),
            "藏宝阁返回没有逐条副属性强化次数；相关字段会保留为未知，不按零次计算。".to_owned(),
        ];
        if platform.is_none() {
            warnings.push("藏宝阁公开详情未提供安卓/iOS平台标识，平台将保持未知。".to_owned());
        }
        let preview = CbgReadPreview {
            source_url: source_url.clone(),
            account_name,
            server_name,
            platform,
            inventory_count,
            retained_count: inventory_count,
            six_star_count,
            lower_star_count,
            shikigami_count,
            sample_souls: souls
                .iter()
                .take(CBG_PREVIEW_SAMPLE_SIZE)
                .map(CbgSoulRecord::preview)
                .collect(),
            warnings: warnings.clone(),
            message: format!(
                "已读取藏宝阁公开数据：{} 枚御魂（六星 {} 枚）、{} 个式神；确认后会覆盖当前御魂与式神录。",
                inventory_count, six_star_count, shikigami_count
            ),
        };
        let payload = json!({
            "format": "cbg-equip-v1",
            "schemaVersion": 1,
            "source": "cbg",
            "sourceUrl": source_url.clone(),
            "serverId": server_id.clone(),
            "orderSn": order_sn.clone(),
            "accountName": preview.account_name.clone(),
            "serverName": preview.server_name.clone(),
            "platform": preview.platform.clone(),
            "completeness": "complete",
            "capturedAt": Utc::now().to_rfc3339(),
            "inventoryCount": inventory_count,
            "sixStarCount": six_star_count,
            "lowerStarCount": lower_star_count,
            "shikigamiCount": shikigami_count,
            "warnings": warnings,
            "souls": souls.iter().map(|soul| soul.normalized.clone()).collect::<Vec<_>>(),
            "heroes": heroes,
        });
        let payload = serde_json::to_vec(&payload)
            .map_err(|error| AppError::internal(format!("生成藏宝阁导入 JSON 失败：{error}")))?;
        ensure_bytes("cbgPayloadBytes", payload.len(), MAX_IMPORT_FILE_BYTES)?;

        Ok(CbgImportPreparation {
            file: ImportFileInput {
                file_name: format!("cbg-{order_sn}.json"),
                payload,
            },
            preview,
        })
    }
}

/// 从藏宝阁详情中提取明确的平台字段；字段缺失或值不在标准枚举内时返回未知。
///
/// 不把 server_id、商品链接或接口默认值当作平台依据，因为同一藏宝阁区服可能同时存在安卓和 iOS 账号。
fn detect_cbg_platform(
    equip: &serde_json::Map<String, Value>,
    equip_desc: &Value,
) -> Option<String> {
    // 不同版本接口字段名略有差异，优先检查商品详情，再检查详情快照顶层。
    const PLATFORM_FIELDS: &[&str] = &[
        "platform",
        "platform_name",
        "platformName",
        "game_platform",
        "gamePlatform",
        "device_platform",
        "devicePlatform",
        "platform_type",
        "platformType",
        "os",
        "os_type",
        "osType",
    ];

    let candidates = [Some(equip), equip_desc.as_object()];
    for object in candidates.into_iter().flatten() {
        for field in PLATFORM_FIELDS {
            if let Some(platform) = object.get(*field).and_then(normalize_cbg_platform) {
                return Some(platform);
            }
        }
    }
    None
}

/// 将藏宝阁平台展示文本收敛为角色档案认可的 android/ios 枚举，其他文本不做猜测。
fn normalize_cbg_platform(value: &Value) -> Option<String> {
    // 藏宝阁详情接口的 platform_type 枚举已验证为 1=iOS、2=安卓；其他数字不做猜测。
    if let Some(code) = value.as_u64() {
        return match code {
            1 => Some(PLATFORM_IOS.to_owned()),
            2 => Some(PLATFORM_ANDROID.to_owned()),
            _ => None,
        };
    }
    let text = value.as_str()?.trim();
    let compact = text.to_ascii_lowercase().replace([' ', '_', '-'], "");
    // 允许“网易安卓”“iOS 端”等展示前后缀，但两个平台同时出现时视为歧义。
    if compact == "1" {
        return Some(PLATFORM_IOS.to_owned());
    }
    if compact == "2" {
        return Some(PLATFORM_ANDROID.to_owned());
    }
    let is_android = compact.contains("android") || compact.contains("安卓");
    let is_ios = compact.contains("ios")
        || compact.contains("iphone")
        || compact.contains("ipad")
        || compact.contains("apple")
        || compact.contains("苹果");
    match (is_android, is_ios) {
        (true, false) => Some(PLATFORM_ANDROID.to_owned()),
        (false, true) => Some(PLATFORM_IOS.to_owned()),
        _ => None,
    }
}

/// 读取藏宝阁商品页主卡片的安卓/iOS图标；网络或页面结构异常时静默返回未知，不影响御魂读取。
fn fetch_cbg_page_platform(source_url: &str) -> Option<String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("YYS-Analysis/0.1 藏宝阁平台读取")
        .build()
        .ok()?;
    let response = client
        .get(source_url)
        .header(reqwest::header::REFERER, source_url)
        // 页面只用于读取平台类名，要求明文响应，避免未启用 gzip 解码时拿到压缩字节。
        .header(reqwest::header::ACCEPT_ENCODING, "identity")
        .send()
        .ok()?
        .error_for_status()
        .ok()?;
    if response
        .content_length()
        .is_some_and(|length| length as usize > MAX_CBG_RESPONSE_BYTES)
    {
        return None;
    }
    let bytes = response.bytes().ok()?;
    if bytes.len() > MAX_CBG_RESPONSE_BYTES {
        return None;
    }
    extract_cbg_platform_from_html(&String::from_utf8_lossy(&bytes))
}

/// 从页面主商品预览区域提取平台类名；限定在主卡片结束前，避免命中“相似推荐”的其他平台图标。
fn extract_cbg_platform_from_html(html: &str) -> Option<String> {
    let preview_start = html.find("pdetailPreview")?;
    let preview = &html[preview_start..];
    let preview_end = preview
        .find("module-selling-info")
        .unwrap_or(preview.len())
        .min(16 * 1024);
    let main_card = &preview[..preview_end];
    match (
        main_card.contains("icon-ios"),
        main_card.contains("icon-android"),
    ) {
        (true, false) => Some(PLATFORM_IOS.to_owned()),
        (false, true) => Some(PLATFORM_ANDROID.to_owned()),
        _ => None,
    }
}

/// 统计公开 heroes 中可进入式神录的记录；字段损坏或特殊角色不计入预览数量。
fn count_importable_cbg_heroes(heroes: &serde_json::Map<String, Value>) -> u32 {
    heroes
        .values()
        .filter(|value| {
            let Some(record) = value.as_object() else {
                return false;
            };
            let Some(shikigami_id) = optional_text(record, "heroId") else {
                return false;
            };
            !is_desktop_special_hero_id(&shikigami_id)
        })
        .count() as u32
}

/// 只允许阴阳师藏宝阁商品详情链接，防止“导入链接”被扩展成任意远程请求入口。
fn parse_cbg_url(source_url: &str) -> Result<(String, String), AppError> {
    let url = Url::parse(source_url).map_err(|error| {
        AppError::invalid_argument("sourceUrl", format!("藏宝阁链接格式不正确：{error}"))
    })?;
    if url.scheme() != "https" || url.host_str() != Some("yys.cbg.163.com") {
        return Err(AppError::invalid_argument(
            "sourceUrl",
            "只支持 https://yys.cbg.163.com 的阴阳师藏宝阁商品链接",
        ));
    }
    let segments = url
        .path_segments()
        .map(|segments| segments.collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.len() != 5 || segments[0..3] != ["cgi", "mweb", "equip"] {
        return Err(AppError::invalid_argument(
            "sourceUrl",
            "链接不是藏宝阁商品详情地址，请复制 /cgi/mweb/equip/ 开头的分享链接",
        ));
    }
    let server_id = segments[3].trim();
    let order_sn = segments[4].trim();
    if server_id.is_empty() || order_sn.is_empty() || order_sn.len() > 128 {
        return Err(AppError::invalid_argument(
            "sourceUrl",
            "藏宝阁商品编号缺失或长度不正确",
        ));
    }
    Ok((server_id.to_owned(), order_sn.to_owned()))
}

/// 通过藏宝阁公开接口获取商品详情；仅发送链接路径中解析出的商品标识。
fn fetch_detail(source_url: &str, server_id: &str, order_sn: &str) -> Result<Value, AppError> {
    let client = Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("YYS-Analysis/0.1 藏宝阁读取")
        .build()
        .map_err(|error| AppError::internal(format!("初始化藏宝阁网络客户端失败：{error}")))?;
    let response = client
        .post(CBG_DETAIL_API)
        .header(reqwest::header::REFERER, source_url)
        .form(&[
            ("serverid", server_id),
            ("ordersn", order_sn),
            ("view_loc", "link_copy"),
        ])
        .send()
        .map_err(|error| {
            AppError::invalid_argument("sourceUrl", format!("无法读取藏宝阁商品：{error}"))
        })?
        .error_for_status()
        .map_err(|error| {
            AppError::invalid_argument("sourceUrl", format!("藏宝阁接口返回失败：{error}"))
        })?;
    if let Some(content_length) = response.content_length() {
        ensure_bytes(
            "cbgResponseBytes",
            content_length as usize,
            MAX_CBG_RESPONSE_BYTES,
        )?;
    }
    let bytes = response.bytes().map_err(|error| {
        AppError::invalid_argument("sourceUrl", format!("读取藏宝阁响应失败：{error}"))
    })?;
    ensure_bytes("cbgResponseBytes", bytes.len(), MAX_CBG_RESPONSE_BYTES)?;
    let response: Value = serde_json::from_slice(&bytes).map_err(|error| {
        AppError::invalid_argument("sourceUrl", format!("藏宝阁响应不是有效 JSON：{error}"))
    })?;
    if response.get("status").and_then(Value::as_i64) != Some(1) {
        let message = response
            .get("msg")
            .and_then(Value::as_str)
            .unwrap_or("商品不存在、已下架或暂时无法读取");
        return Err(AppError::not_found("藏宝阁商品", message));
    }
    Ok(response)
}

/// 中间态御魂同时保存标准快照对象和页面预览所需的原始展示文本。
struct CbgSoulRecord {
    normalized: Value,
    id: String,
    set_name: String,
    slot: u8,
    quality: u8,
    level: u8,
    main_attribute: String,
    main_value: String,
    sub_attributes: Vec<String>,
}

impl CbgSoulRecord {
    /// 将中间态转成少量可展示字段，避免前端接触藏宝阁原始对象。
    fn preview(&self) -> CbgSoulPreview {
        CbgSoulPreview {
            id: self.id.clone(),
            set_name: self.set_name.clone(),
            slot: self.slot,
            quality: self.quality,
            level: self.level,
            main_attribute: self.main_attribute.clone(),
            main_value: self.main_value.clone(),
            sub_attributes: self.sub_attributes.clone(),
        }
    }
}

/// 将藏宝阁一条压缩御魂记录转换成导入器认可的标准字段。
fn parse_cbg_soul(fallback_id: &str, value: &Value) -> Result<CbgSoulRecord, AppError> {
    let file_name = "藏宝阁公开御魂库存";
    let path = format!("$.inventory.{fallback_id}");
    let object = value.as_object().ok_or_else(|| {
        AppError::import_field(
            file_name,
            &path,
            "御魂对象",
            value_kind(value),
            "藏宝阁御魂记录不是对象",
        )
    })?;
    let id = optional_text(object, "uuid").unwrap_or_else(|| fallback_id.to_owned());
    let set_name = optional_text(object, "name")
        .or_else(|| {
            optional_u64(object, "suitid")
                .and_then(|code| crate::domain::builtin_soul_set_name_by_code(code % 300_000))
                .map(str::to_owned)
        })
        .ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{path}.name"),
                "套装名称",
                "缺失",
                "藏宝阁御魂缺少套装名称",
            )
        })?;
    let slot = required_u8(object, "pos", &path)?;
    if !(1..=6).contains(&slot) {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.pos"),
            "1 到 6",
            &slot.to_string(),
            "藏宝阁御魂号位超出范围",
        ));
    }
    let quality = required_u8(object, "qua", &path)?;
    if !(1..=6).contains(&quality) {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.qua"),
            "1 到 6",
            &quality.to_string(),
            "藏宝阁御魂星级超出范围",
        ));
    }
    let level = required_u8(object, "level", &path)?;
    if level > 15 {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.level"),
            "0 到 15",
            &level.to_string(),
            "藏宝阁御魂等级超出范围",
        ));
    }
    let attrs = object
        .get("attrs")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            AppError::import_field(
                file_name,
                &format!("{path}.attrs"),
                "属性数组",
                "缺失",
                "藏宝阁御魂缺少属性数组",
            )
        })?;
    if attrs.is_empty() {
        return Err(AppError::import_field(
            file_name,
            &format!("{path}.attrs"),
            "至少 1 条主属性",
            "空数组",
            "藏宝阁御魂属性数组为空",
        ));
    }
    let parsed_attrs = attrs
        .iter()
        .enumerate()
        .map(|(index, attr)| parse_cbg_attribute(attr, &format!("{path}.attrs[{index}]")))
        .collect::<Result<Vec<_>, _>>()?;
    let (main_type, main_value, main_label) = cbg_main_attribute(
        slot,
        &parsed_attrs[0].0,
        &parsed_attrs[0].1,
        &parsed_attrs[0].2,
    );
    let mut sub_attributes = Vec::with_capacity(parsed_attrs.len().saturating_sub(1));
    let mut sub_values = Vec::with_capacity(parsed_attrs.len().saturating_sub(1));
    for (name, raw_value, display_value) in parsed_attrs.iter().skip(1) {
        let attribute_type = cbg_attribute_type(name, display_value);
        let normalized_value = cbg_normalize_value(&attribute_type, raw_value);
        sub_attributes.push(format!("{name} {display_value}"));
        sub_values.push(json!({"type": attribute_type, "value": normalized_value}));
    }
    // 只有未强化御魂的初始副属性数量可从当前公开数组可靠推断；已强化御魂的强化落点仍保持未知。
    let initial_substat_count = if level == 0 {
        Some(sub_values.len() as u8)
    } else {
        None
    };
    let normalized = json!({
        "id": id.clone(),
        "setId": set_name.clone(),
        "slot": slot,
        "quality": quality,
        "level": level,
        "mainAttr": {"type": main_type, "value": main_value},
        "subAttributes": sub_values,
        "initialSubstatCount": initial_substat_count,
        "locked": optional_bool(object, "lock"),
    });
    Ok(CbgSoulRecord {
        normalized,
        id,
        set_name,
        slot,
        quality,
        level,
        main_attribute: main_label,
        main_value: parsed_attrs[0].2.clone(),
        sub_attributes,
    })
}

/// 读取藏宝阁 attrs 的二元数组；同时保留带百分号的原始展示文本，修正“攻击/生命”等简写歧义。
fn parse_cbg_attribute(value: &Value, path: &str) -> Result<(String, f64, String), AppError> {
    let pair = value.as_array().ok_or_else(|| {
        AppError::import_field(
            "藏宝阁公开御魂库存",
            path,
            "[属性名, 数值]",
            value_kind(value),
            "藏宝阁属性不是二元数组",
        )
    })?;
    if pair.len() != 2 {
        return Err(AppError::import_field(
            "藏宝阁公开御魂库存",
            path,
            "长度为 2",
            &pair.len().to_string(),
            "藏宝阁属性数组长度不正确",
        ));
    }
    let name = pair[0].as_str().ok_or_else(|| {
        AppError::import_field(
            "藏宝阁公开御魂库存",
            path,
            "属性名字符串",
            value_kind(&pair[0]),
            "藏宝阁属性名称类型不正确",
        )
    })?;
    let display_value = match &pair[1] {
        Value::String(value) => value.clone(),
        value => value.to_string(),
    };
    let raw_value = parse_number(&display_value).ok_or_else(|| {
        AppError::import_field(
            "藏宝阁公开御魂库存",
            path,
            "数字属性值",
            &display_value,
            "藏宝阁属性数值类型不正确",
        )
    })?;
    Ok((name.to_owned(), raw_value, display_value))
}

/// 按藏宝阁展示值判断简写属性是固定值还是百分比，避免把“生命 306”解析成生命加成。
fn cbg_attribute_type(name: &str, display_value: &str) -> String {
    let compact = name.trim().replace(['_', '-', ' ', '%'], "");
    let is_percent = display_value.contains('%');
    match compact.as_str() {
        "攻击" | "攻击加成" => {
            if is_percent {
                "attack_rate"
            } else {
                "attack_flat"
            }
        }
        "防御" | "防御加成" => {
            if is_percent {
                "defense_rate"
            } else {
                "defense_flat"
            }
        }
        "生命" | "生命加成" => {
            if is_percent {
                "hp_rate"
            } else {
                "hp_flat"
            }
        }
        "速度" => "speed",
        "暴击" | "暴击率" => "crit_rate",
        "暴伤" | "暴击伤害" => "crit_damage",
        "命中" | "效果命中" => "effect_hit",
        "抵抗" | "效果抵抗" => "effect_resist",
        _ => name.trim(),
    }
    .to_owned()
}

/// 主属性号位有确定语义；在此把藏宝阁主属性名称转换成稳定属性 ID。
fn cbg_main_attribute(
    slot: u8,
    name: &str,
    raw_value: &f64,
    display_value: &str,
) -> (String, f64, String) {
    // 非固定号位的主属性需要先保存推导结果，避免 match 分支借用临时 String。
    let derived_attribute_type = cbg_attribute_type(name, display_value);
    let attribute_type = match slot {
        1 => "attack_flat",
        3 => "defense_flat",
        5 => "hp_flat",
        _ => derived_attribute_type.as_str(),
    }
    .to_owned();
    let normalized_value = cbg_normalize_value(&attribute_type, raw_value);
    (attribute_type, normalized_value, name.to_owned())
}

/// 百分比属性统一转换为内部小数；固定攻击/生命等保持游戏展示数值。
fn cbg_normalize_value(attribute_type: &str, value: &f64) -> f64 {
    if matches!(
        attribute_type,
        "attack_rate"
            | "hp_rate"
            | "defense_rate"
            | "crit_rate"
            | "crit_damage"
            | "effect_hit"
            | "effect_resist"
    ) && value.abs() > 1.0
    {
        value / 100.0
    } else {
        *value
    }
}

/// 兼容藏宝阁返回的数字和数字字符串；百分号只影响解析，不把符号写回内部事实。
fn parse_number(value: &str) -> Option<f64> {
    value
        .trim()
        .trim_end_matches('%')
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite())
}

/// 读取对象中的文本或数字字段，用于兼容藏宝阁不同版本的返回类型。
fn optional_text(object: &serde_json::Map<String, Value>, field: &str) -> Option<String> {
    object.get(field).and_then(|value| match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    })
}

/// 读取对象中的非负整数；字段类型异常时由调用方转换成导入错误。
fn optional_u64(object: &serde_json::Map<String, Value>, field: &str) -> Option<u64> {
    object.get(field).and_then(|value| match value {
        Value::Number(value) => value.as_u64(),
        Value::String(value) => value.parse::<u64>().ok(),
        _ => None,
    })
}

/// 读取藏宝阁的 0/1 锁定字段；未知类型按未知处理，不伪造锁定状态。
fn optional_bool(object: &serde_json::Map<String, Value>, field: &str) -> Option<bool> {
    object.get(field).and_then(|value| match value {
        Value::Bool(value) => Some(*value),
        Value::Number(value) => value.as_i64().map(|value| value != 0),
        Value::String(value) => match value.as_str() {
            "1" | "true" => Some(true),
            "0" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    })
}

/// 读取必需的 0—255 整数，并把错误定位到具体藏宝阁字段。
fn required_u8(
    object: &serde_json::Map<String, Value>,
    field: &str,
    path: &str,
) -> Result<u8, AppError> {
    optional_u64(object, field)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or_else(|| {
            AppError::import_field(
                "藏宝阁公开御魂库存",
                &format!("{path}.{field}"),
                "非负整数",
                "缺失或类型错误",
                "藏宝阁御魂字段类型不正确",
            )
        })
}

/// 将 JSON 值转为短类型名，避免把原始御魂正文写进错误消息。
fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 用用户提供链接中实际出现的 attrs 形状回归主属性号位和百分比副属性转换。
    #[test]
    fn 藏宝阁样例御魂应映射到标准字段() {
        let raw = json!({
            "uuid": "6a1c0e41f9e6d8f96ddbf23b",
            "name": "招财猫",
            "pos": 1,
            "qua": 6,
            "level": 15,
            "lock": 1,
            "attrs": [
                ["攻击", "486.00"],
                ["生命", "306.00"],
                ["速度", "5.26"],
                ["暴击伤害", "7.16%"],
                ["效果抵抗", "7.18%"]
            ]
        });

        let parsed = parse_cbg_soul("fallback", &raw).expect("样例御魂应能解析");
        assert_eq!(parsed.id, "6a1c0e41f9e6d8f96ddbf23b");
        assert_eq!(parsed.set_name, "招财猫");
        assert_eq!(parsed.normalized["mainAttr"]["type"], "attack_flat");
        assert_eq!(parsed.normalized["subAttributes"][0]["type"], "hp_flat");
        assert_eq!(parsed.normalized["subAttributes"][2]["value"], 0.0716);
        assert_eq!(parsed.normalized["locked"], true);
        assert_eq!(parsed.normalized["initialSubstatCount"], Value::Null);
    }

    /// 预检统计只计算有稳定 heroId 的普通式神，避免把特殊角色和损坏记录展示成可导入数量。
    #[test]
    fn 藏宝阁式神预览应排除特殊与损坏记录() {
        let raw = json!({
            "normal": {"heroId": 398},
            "special": {"heroId": 10},
            "broken": {"name": "缺少 ID"},
            "not-an-object": "invalid"
        });
        let heroes = raw.as_object().expect("式神映射应为对象");
        assert_eq!(count_importable_cbg_heroes(heroes), 1);
    }

    /// 平台只接受藏宝阁详情中的显式安卓/iOS文本，未知值和缺失字段保持未知。
    #[test]
    fn 藏宝阁平台应读取显式标识且不根据区服猜测() {
        let android = json!({"platform_name": "安卓端"});
        let android_object = android.as_object().expect("详情应为对象");
        assert_eq!(
            detect_cbg_platform(android_object, &json!({})),
            Some(PLATFORM_ANDROID.to_owned())
        );

        let ios_code = json!({"platform_type": 1});
        assert_eq!(
            detect_cbg_platform(ios_code.as_object().expect("详情应为对象"), &json!({}),),
            Some(PLATFORM_IOS.to_owned())
        );
        let android_code = json!({"platform_type": 2});
        assert_eq!(
            detect_cbg_platform(android_code.as_object().expect("详情应为对象"), &json!({}),),
            Some(PLATFORM_ANDROID.to_owned())
        );

        let ios = json!({"gamePlatform": "iOS"});
        let ios_object = ios.as_object().expect("详情应为对象");
        assert_eq!(
            detect_cbg_platform(&serde_json::Map::new(), &json!({"gamePlatform": "iOS"})),
            Some(PLATFORM_IOS.to_owned())
        );
        assert_eq!(
            detect_cbg_platform(ios_object, &json!({"server_id": 3})),
            Some(PLATFORM_IOS.to_owned())
        );
        assert_eq!(
            detect_cbg_platform(&serde_json::Map::new(), &json!({"platform": "未知平台"})),
            None
        );
    }

    /// 商品页主卡片的图标应能识别平台，且主卡片缺失图标或同时出现两种图标时保持未知。
    #[test]
    fn 藏宝阁商品页应只读取主卡片平台图标() {
        let ios_html = r#"
            <div class="list-block border pdetailPreview">
              <span class="icon s-l icon-ios"></span>
              <div class="module-selling-info"></div>
            </div>
            <span class="icon icon-android"></span>
        "#;
        assert_eq!(
            extract_cbg_platform_from_html(ios_html),
            Some(PLATFORM_IOS.to_owned())
        );

        let android_html = r#"
            <div class="pdetailPreview"><span class="icon-android"></span></div>
            <div class="module-selling-info"></div>
        "#;
        assert_eq!(
            extract_cbg_platform_from_html(android_html),
            Some(PLATFORM_ANDROID.to_owned())
        );
        assert_eq!(
            extract_cbg_platform_from_html(
                r#"<div class="pdetailPreview"><span class="icon-ios"></span><span class="icon-android"></span></div>"#,
            ),
            None
        );
    }

    /// 链接解析只允许指定藏宝阁域名和商品路径，查询参数保持可兼容但不参与请求标识。
    #[test]
    fn 藏宝阁链接应提取服务器和商品编号() {
        let (server_id, order_sn) = parse_cbg_url(
            "https://yys.cbg.163.com/cgi/mweb/equip/3/202607132201616-3-MCURJKTD8PY8JP?view_loc=link_copy",
        )
        .expect("藏宝阁商品链接应能解析");
        assert_eq!(server_id, "3");
        assert_eq!(order_sn, "202607132201616-3-MCURJKTD8PY8JP");
        assert!(parse_cbg_url("https://example.com/cgi/mweb/equip/3/order").is_err());
    }
}
