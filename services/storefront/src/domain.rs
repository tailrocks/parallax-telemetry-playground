//! Commerce domain projections and Juniper value objects.

use anyhow::{Context as _, anyhow};
use juniper::{GraphQLInputObject, graphql_object};
use playground_proto::pricing::v1::{
    Money as ProtoMoney, QuoteLine as ProtoQuoteLine, QuoteResponse, QuoteStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    application::StoreContext,
    infrastructure::{
        CatalogCategory, CatalogPriceSnapshot, CatalogProduct, CatalogProductPage,
        CatalogProductVariant, CatalogReview,
    },
};

#[derive(Clone, Debug)]
pub(crate) struct Category {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) slug: String,
    pub(crate) name: String,
}

#[graphql_object(context = StoreContext)]
impl Category {
    fn id(&self) -> &str {
        &self.id
    }
    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    fn slug(&self) -> &str {
        &self.slug
    }
    fn name(&self) -> &str {
        &self.name
    }
}

impl Category {
    pub(crate) fn from_catalog(value: CatalogCategory) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            slug: value.slug,
            name: value.name,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PriceSnapshot {
    pub(crate) id: String,
    pub(crate) currency: String,
    pub(crate) amount_minor: i32,
    pub(crate) compare_at_minor: Option<i32>,
    pub(crate) valid_from: String,
}

#[graphql_object(context = StoreContext)]
impl PriceSnapshot {
    fn id(&self) -> &str {
        &self.id
    }
    fn currency(&self) -> &str {
        &self.currency
    }
    fn amount_minor(&self) -> i32 {
        self.amount_minor
    }
    fn compare_at_minor(&self) -> Option<i32> {
        self.compare_at_minor
    }
    fn valid_from(&self) -> &str {
        &self.valid_from
    }
}

impl From<CatalogPriceSnapshot> for PriceSnapshot {
    fn from(value: CatalogPriceSnapshot) -> Self {
        Self {
            id: value.id,
            currency: value.currency,
            amount_minor: value.amount_minor,
            compare_at_minor: value.compare_at_minor,
            valid_from: value.valid_from,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProductVariant {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) product_id: String,
    pub(crate) sku: String,
    pub(crate) name: String,
    pub(crate) options: String,
    pub(crate) price: Option<PriceSnapshot>,
}

#[graphql_object(context = StoreContext)]
impl ProductVariant {
    fn id(&self) -> &str {
        &self.id
    }
    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    fn product_id(&self) -> &str {
        &self.product_id
    }
    fn sku(&self) -> &str {
        &self.sku
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn options(&self) -> &str {
        &self.options
    }
    fn price(&self) -> Option<&PriceSnapshot> {
        self.price.as_ref()
    }
}

impl From<CatalogProductVariant> for ProductVariant {
    fn from(value: CatalogProductVariant) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            product_id: value.product_id,
            sku: value.sku,
            name: value.name,
            options: value.options,
            price: value.price.map(PriceSnapshot::from),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Review {
    pub(crate) id: String,
    pub(crate) product_id: String,
    pub(crate) title: String,
    pub(crate) text: String,
    pub(crate) stars: i32,
    pub(crate) verified_purchase: bool,
    pub(crate) created_at: String,
}

#[graphql_object(context = StoreContext)]
impl Review {
    fn id(&self) -> &str {
        &self.id
    }
    fn product_id(&self) -> &str {
        &self.product_id
    }
    fn title(&self) -> &str {
        &self.title
    }
    fn text(&self) -> &str {
        &self.text
    }
    fn stars(&self) -> i32 {
        self.stars
    }
    fn verified_purchase(&self) -> bool {
        self.verified_purchase
    }
    fn created_at(&self) -> &str {
        &self.created_at
    }
}

impl From<CatalogReview> for Review {
    fn from(value: CatalogReview) -> Self {
        Self {
            id: value.id,
            product_id: value.product_id,
            title: value.title,
            text: value.text,
            stars: value.stars,
            verified_purchase: value.verified_purchase,
            created_at: value.created_at,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Product {
    pub(crate) id: String,
    pub(crate) tenant_id: String,
    pub(crate) slug: String,
    pub(crate) sku: String,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) brand: Option<String>,
    pub(crate) category: Category,
    pub(crate) price_minor: Option<i32>,
    pub(crate) price: Option<PriceSnapshot>,
    pub(crate) variants: Vec<ProductVariant>,
    pub(crate) reviews: Vec<Review>,
    pub(crate) reviews_slow: Vec<Review>,
    pub(crate) risk_score: Option<f64>,
}

#[graphql_object(context = StoreContext)]
impl Product {
    fn id(&self) -> &str {
        &self.id
    }
    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    fn slug(&self) -> &str {
        &self.slug
    }
    fn sku(&self) -> &str {
        &self.sku
    }
    fn name(&self) -> &str {
        &self.name
    }
    fn description(&self) -> &str {
        &self.description
    }
    fn brand(&self) -> Option<&str> {
        self.brand.as_deref()
    }
    fn category(&self) -> &Category {
        &self.category
    }
    fn price_minor(&self) -> Option<i32> {
        self.price_minor
    }
    fn price(&self) -> Option<&PriceSnapshot> {
        self.price.as_ref()
    }
    fn variants(&self) -> &[ProductVariant] {
        &self.variants
    }
    fn reviews(&self) -> &[Review] {
        &self.reviews
    }
    fn reviews_slow(&self) -> &[Review] {
        &self.reviews_slow
    }
    fn risk_score(&self) -> Option<f64> {
        self.risk_score
    }
}

impl Product {
    pub(crate) fn from_catalog(value: CatalogProduct) -> Self {
        Self {
            id: value.id,
            tenant_id: value.tenant_id,
            slug: value.slug,
            sku: value.sku,
            name: value.name,
            description: value.description,
            brand: value.brand,
            category: Category::from_catalog(value.category),
            price_minor: value.price_minor,
            price: value.price.map(PriceSnapshot::from),
            variants: value
                .variants
                .into_iter()
                .map(ProductVariant::from)
                .collect(),
            reviews: value.reviews.into_iter().map(Review::from).collect(),
            reviews_slow: value.reviews_slow.into_iter().map(Review::from).collect(),
            risk_score: value.risk_score,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ProductPage {
    pub(crate) items: Vec<Product>,
    pub(crate) page: i32,
    pub(crate) size: i32,
    pub(crate) total_elements: i32,
    pub(crate) total_pages: i32,
    pub(crate) has_next: bool,
    pub(crate) experience: String,
}

#[graphql_object(context = StoreContext)]
impl ProductPage {
    fn items(&self) -> &[Product] {
        &self.items
    }
    fn page(&self) -> i32 {
        self.page
    }
    fn size(&self) -> i32 {
        self.size
    }
    fn total_elements(&self) -> i32 {
        self.total_elements
    }
    fn total_pages(&self) -> i32 {
        self.total_pages
    }
    fn has_next(&self) -> bool {
        self.has_next
    }
    fn experience(&self) -> &str {
        &self.experience
    }
}

impl ProductPage {
    pub(crate) fn from_catalog(value: CatalogProductPage) -> Self {
        Self {
            items: value.items.into_iter().map(Product::from_catalog).collect(),
            page: value.page,
            size: value.size,
            total_elements: value.total_elements,
            total_pages: value.total_pages,
            has_next: value.has_next,
            experience: value.experience,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct PriceChange {
    pub(crate) product: Product,
    pub(crate) variant: ProductVariant,
    pub(crate) price: PriceSnapshot,
    pub(crate) observed_at: String,
}

#[graphql_object(context = StoreContext)]
impl PriceChange {
    fn product(&self) -> &Product {
        &self.product
    }
    fn variant(&self) -> &ProductVariant {
        &self.variant
    }
    fn price(&self) -> &PriceSnapshot {
        &self.price
    }
    fn observed_at(&self) -> &str {
        &self.observed_at
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Money {
    pub(crate) currency_code: String,
    pub(crate) amount_minor: i64,
}

#[graphql_object(context = StoreContext)]
impl Money {
    fn currency_code(&self) -> &str {
        &self.currency_code
    }
    fn amount_minor(&self) -> String {
        self.amount_minor.to_string()
    }
}

impl From<ProtoMoney> for Money {
    fn from(value: ProtoMoney) -> Self {
        Self {
            currency_code: value.currency_code,
            amount_minor: value.amount_minor,
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct QuoteLine {
    pub(crate) sku: String,
    pub(crate) quantity: i32,
    pub(crate) unit_price: Option<Money>,
    pub(crate) line_total: Option<Money>,
}

#[graphql_object(context = StoreContext)]
impl QuoteLine {
    fn sku(&self) -> &str {
        &self.sku
    }
    fn quantity(&self) -> i32 {
        self.quantity
    }
    fn unit_price(&self) -> Option<&Money> {
        self.unit_price.as_ref()
    }
    fn line_total(&self) -> Option<&Money> {
        self.line_total.as_ref()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Quote {
    pub(crate) quote_id: String,
    pub(crate) status: String,
    pub(crate) lines: Vec<QuoteLine>,
    pub(crate) subtotal: Option<Money>,
    pub(crate) discount_total: Option<Money>,
    pub(crate) tax_total: Option<Money>,
    pub(crate) grand_total: Option<Money>,
    pub(crate) valid_for_seconds: i32,
    pub(crate) pricing_version: String,
}

#[graphql_object(context = StoreContext)]
impl Quote {
    fn quote_id(&self) -> &str {
        &self.quote_id
    }
    fn status(&self) -> &str {
        &self.status
    }
    fn lines(&self) -> &[QuoteLine] {
        &self.lines
    }
    fn subtotal(&self) -> Option<&Money> {
        self.subtotal.as_ref()
    }
    fn discount_total(&self) -> Option<&Money> {
        self.discount_total.as_ref()
    }
    fn tax_total(&self) -> Option<&Money> {
        self.tax_total.as_ref()
    }
    fn grand_total(&self) -> Option<&Money> {
        self.grand_total.as_ref()
    }
    fn valid_for_seconds(&self) -> i32 {
        self.valid_for_seconds
    }
    fn pricing_version(&self) -> &str {
        &self.pricing_version
    }
}

impl Quote {
    pub(crate) fn try_from_proto(value: QuoteResponse) -> anyhow::Result<Self> {
        let status =
            QuoteStatus::try_from(value.status).map_err(|_| anyhow!("quote status is invalid"))?;
        if status != QuoteStatus::Ready {
            return Err(anyhow!("quote is not ready"));
        }
        if value.quote_id.trim().is_empty() || value.pricing_version.trim().is_empty() {
            return Err(anyhow!("quote identity is incomplete"));
        }
        if value.valid_for_seconds == 0 {
            return Err(anyhow!("quote expiration is missing"));
        }
        if value.lines.is_empty() || value.lines.len() > 50 {
            return Err(anyhow!("quote line count is invalid"));
        }
        let lines = value
            .lines
            .into_iter()
            .map(QuoteLine::try_from_proto)
            .collect::<anyhow::Result<Vec<_>>>()?;
        let subtotal = required_money(value.subtotal, "subtotal")?;
        let discount_total = required_money(value.discount_total, "discount_total")?;
        let tax_total = required_money(value.tax_total, "tax_total")?;
        let grand_total = required_money(value.grand_total, "grand_total")?;
        for money in [&discount_total, &tax_total, &grand_total] {
            if money.currency_code != subtotal.currency_code {
                return Err(anyhow!("quote currencies do not match"));
            }
        }
        let line_subtotal = lines.iter().try_fold(0_i64, |total, line| {
            let line_total = line
                .line_total
                .as_ref()
                .ok_or_else(|| anyhow!("quote line total is missing"))?;
            total
                .checked_add(line_total.amount_minor)
                .ok_or_else(|| anyhow!("quote subtotal overflowed"))
        })?;
        if line_subtotal != subtotal.amount_minor
            || discount_total.amount_minor > subtotal.amount_minor
        {
            return Err(anyhow!("quote totals do not match quote lines"));
        }
        let expected_total = subtotal
            .amount_minor
            .checked_sub(discount_total.amount_minor)
            .and_then(|total| total.checked_add(tax_total.amount_minor))
            .ok_or_else(|| anyhow!("quote total arithmetic overflowed"))?;
        if expected_total != grand_total.amount_minor {
            return Err(anyhow!("quote totals do not balance"));
        }
        Ok(Self {
            quote_id: value.quote_id,
            status: status.as_str_name().to_owned(),
            lines,
            subtotal: Some(subtotal),
            discount_total: Some(discount_total),
            tax_total: Some(tax_total),
            grand_total: Some(grand_total),
            valid_for_seconds: i32::try_from(value.valid_for_seconds)
                .context("quote validity exceeds GraphQL Int")?,
            pricing_version: value.pricing_version,
        })
    }
}

impl QuoteLine {
    pub(crate) fn try_from_proto(value: ProtoQuoteLine) -> anyhow::Result<Self> {
        if value.sku.trim().is_empty() || value.quantity == 0 {
            return Err(anyhow!("quote line identity or quantity is invalid"));
        }
        let unit_price = required_money(value.unit_price, "line unit_price")?;
        let line_total = required_money(value.line_total, "line line_total")?;
        if unit_price.currency_code != line_total.currency_code
            || unit_price
                .amount_minor
                .checked_mul(i64::from(value.quantity))
                != Some(line_total.amount_minor)
        {
            return Err(anyhow!("quote line amount does not match quantity"));
        }
        Ok(Self {
            sku: value.sku,
            quantity: i32::try_from(value.quantity)
                .context("quote quantity exceeds GraphQL Int")?,
            unit_price: Some(unit_price),
            line_total: Some(line_total),
        })
    }
}

fn required_money(value: Option<ProtoMoney>, name: &str) -> anyhow::Result<Money> {
    let money = value.ok_or_else(|| anyhow!("quote {name} is missing"))?;
    if money.currency_code.len() != 3
        || money.currency_code != money.currency_code.to_ascii_uppercase()
        || !money
            .currency_code
            .bytes()
            .all(|byte| byte.is_ascii_alphabetic())
        || money.amount_minor < 0
    {
        return Err(anyhow!("quote {name} is invalid"));
    }
    Ok(Money::from(money))
}

#[derive(Clone, Debug, GraphQLInputObject, Deserialize, Serialize)]
#[graphql(name = "CartItemInput")]
pub(crate) struct CartItemInput {
    pub(crate) sku: String,
    pub(crate) quantity: i32,
}

#[derive(Clone, Debug, GraphQLInputObject, Deserialize, Serialize)]
pub(crate) struct QuoteInput {
    pub(crate) items: Vec<CartItemInput>,
    pub(crate) tenant_id: Option<String>,
    pub(crate) customer_id: Option<String>,
    pub(crate) currency_code: Option<String>,
    pub(crate) promotion_code: Option<String>,
    pub(crate) pricing_strategy: Option<String>,
    pub(crate) payment_method_type: Option<String>,
    pub(crate) segment: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) request_id: Option<String>,
}

#[derive(Clone, Debug, GraphQLInputObject, Deserialize, Serialize)]
pub(crate) struct CheckoutInput {
    pub(crate) items: Vec<CartItemInput>,
    pub(crate) tenant_id: Option<String>,
    pub(crate) customer_id: Option<String>,
    pub(crate) cart_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) currency_code: Option<String>,
    pub(crate) promotion_code: Option<String>,
    pub(crate) pricing_strategy: Option<String>,
    pub(crate) payment_method_token: Option<String>,
    pub(crate) payment_method_type: Option<String>,
    pub(crate) segment: Option<String>,
    pub(crate) tier: Option<String>,
    pub(crate) region: Option<String>,
    pub(crate) priority: Option<String>,
    pub(crate) request_id: Option<String>,
}

#[derive(Clone, Debug, GraphQLInputObject, Deserialize, Serialize)]
#[graphql(name = "AddCartItemInput")]
pub(crate) struct AddCartItemInput {
    pub(crate) sku: String,
    pub(crate) quantity: i32,
    pub(crate) tenant_id: Option<String>,
    pub(crate) customer_id: Option<String>,
    pub(crate) cart_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) currency_code: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct Cart {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) currency: String,
    pub(crate) items: Vec<CartItem>,
}

#[graphql_object(context = StoreContext)]
impl Cart {
    fn id(&self) -> &str {
        &self.id
    }
    fn status(&self) -> &str {
        &self.status
    }
    fn currency(&self) -> &str {
        &self.currency
    }
    fn items(&self) -> &[CartItem] {
        &self.items
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct CartItem {
    pub(crate) sku: String,
    pub(crate) product_name: String,
    pub(crate) quantity: i32,
    pub(crate) unit_price_minor: i32,
    pub(crate) line_total_minor: i32,
}

#[graphql_object(context = StoreContext)]
impl CartItem {
    fn sku(&self) -> &str {
        &self.sku
    }
    fn product_name(&self) -> &str {
        &self.product_name
    }
    fn quantity(&self) -> i32 {
        self.quantity
    }
    fn unit_price_minor(&self) -> i32 {
        self.unit_price_minor
    }
    fn line_total_minor(&self) -> i32 {
        self.line_total_minor
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct CartItemAdded {
    pub(crate) cart_id: String,
    pub(crate) sku: String,
    pub(crate) quantity_added: i32,
    pub(crate) unit_price_minor: i32,
}

#[graphql_object(context = StoreContext)]
impl CartItemAdded {
    fn cart_id(&self) -> &str {
        &self.cart_id
    }
    fn sku(&self) -> &str {
        &self.sku
    }
    fn quantity_added(&self) -> i32 {
        self.quantity_added
    }
    fn unit_price_minor(&self) -> i32 {
        self.unit_price_minor
    }
}

impl CartItemAdded {
    pub(crate) fn from_value(value: &Value) -> anyhow::Result<Self> {
        serde_json::from_value(value.clone())
            .context("cart item add returned an invalid durable cart payload")
    }
}

#[derive(Clone, Debug, GraphQLInputObject, Deserialize, Serialize)]
pub(crate) struct AnalyticsInput {
    pub(crate) tenant_id: String,
    pub(crate) event_key: String,
    pub(crate) event_name: String,
    pub(crate) entity_type: String,
    pub(crate) entity_id: String,
    pub(crate) customer_id: Option<String>,
    pub(crate) session_id: Option<String>,
    pub(crate) occurred_at: String,
    pub(crate) properties: Option<String>,
    pub(crate) context: Option<String>,
}

pub(crate) fn deterministic_event_id(tenant_id: &str, event_key: &str) -> String {
    let mut identity = Vec::with_capacity(24 + tenant_id.len() + event_key.len());
    identity.extend_from_slice(b"parallax.analytics.v1\0");
    identity.extend_from_slice(tenant_id.as_bytes());
    identity.push(0);
    identity.extend_from_slice(event_key.as_bytes());

    let digest = Sha256::digest(identity);
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    uuid::Uuid::from_bytes(bytes).to_string()
}

#[derive(Clone, Debug)]
pub(crate) struct CheckoutResult {
    pub(crate) order_id: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) payment_status: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) total_minor: Option<String>,
    pub(crate) event_key: Option<String>,
    pub(crate) feature_variant: Option<String>,
}

#[graphql_object(context = StoreContext)]
impl CheckoutResult {
    fn order_id(&self) -> Option<&str> {
        self.order_id.as_deref()
    }
    fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    fn payment_status(&self) -> Option<&str> {
        self.payment_status.as_deref()
    }
    fn currency(&self) -> Option<&str> {
        self.currency.as_deref()
    }
    fn total_minor(&self) -> Option<&str> {
        self.total_minor.as_deref()
    }
    fn event_key(&self) -> Option<&str> {
        self.event_key.as_deref()
    }
    fn feature_variant(&self) -> Option<&str> {
        self.feature_variant.as_deref()
    }
}

impl CheckoutResult {
    pub(crate) fn try_from_value(value: &Value) -> anyhow::Result<Self> {
        Ok(Self {
            order_id: Some(required_string_field(value, "order_id")?),
            status: Some(required_string_field(value, "status")?),
            payment_status: optional_string_field(value, "payment_status")?,
            currency: optional_string_field(value, "currency")?,
            total_minor: optional_numeric_string_field(value, "total_minor")?,
            event_key: optional_string_field(value, "event_key")?,
            feature_variant: optional_string_field(value, "feature_variant")?,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AnalyticsAck {
    pub(crate) event_key: String,
    pub(crate) status: String,
}

#[graphql_object(context = StoreContext)]
impl AnalyticsAck {
    fn event_key(&self) -> &str {
        &self.event_key
    }
    fn status(&self) -> &str {
        &self.status
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) struct AnalyticsEvent {
    pub(crate) event_id: String,
    pub(crate) tenant_id: String,
    pub(crate) event_key: String,
    pub(crate) customer_id: Option<String>,
    pub(crate) event_name: String,
    pub(crate) event_version: i32,
    pub(crate) source: String,
    pub(crate) entity_type: String,
    pub(crate) entity_id: String,
    pub(crate) occurred_at: String,
    pub(crate) trace_id: String,
    pub(crate) span_id: String,
    pub(crate) traceparent: String,
    pub(crate) tracestate: String,
    pub(crate) baggage: String,
    pub(crate) feature_variant: String,
    pub(crate) properties: String,
    pub(crate) context: String,
}

#[derive(Clone, Debug)]
pub(crate) struct AnalyticsSummary {
    pub(crate) tenant_id: String,
    pub(crate) event_name: Option<String>,
    pub(crate) event_count: i32,
    pub(crate) unique_customers: i32,
    pub(crate) first_occurred_at: Option<String>,
    pub(crate) last_occurred_at: Option<String>,
}

#[graphql_object(context = StoreContext)]
impl AnalyticsSummary {
    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    fn event_name(&self) -> Option<&str> {
        self.event_name.as_deref()
    }
    fn event_count(&self) -> i32 {
        self.event_count
    }
    fn unique_customers(&self) -> i32 {
        self.unique_customers
    }
    fn first_occurred_at(&self) -> Option<&str> {
        self.first_occurred_at.as_deref()
    }
    fn last_occurred_at(&self) -> Option<&str> {
        self.last_occurred_at.as_deref()
    }
}

#[graphql_object(context = StoreContext)]
impl AnalyticsEvent {
    fn event_id(&self) -> &str {
        &self.event_id
    }
    fn tenant_id(&self) -> &str {
        &self.tenant_id
    }
    fn event_key(&self) -> &str {
        &self.event_key
    }
    fn customer_id(&self) -> Option<&str> {
        self.customer_id.as_deref()
    }
    fn event_name(&self) -> &str {
        &self.event_name
    }
    fn event_version(&self) -> i32 {
        self.event_version
    }
    fn source(&self) -> &str {
        &self.source
    }
    fn entity_type(&self) -> &str {
        &self.entity_type
    }
    fn entity_id(&self) -> &str {
        &self.entity_id
    }
    fn occurred_at(&self) -> &str {
        &self.occurred_at
    }
    fn trace_id(&self) -> &str {
        &self.trace_id
    }
    fn span_id(&self) -> &str {
        &self.span_id
    }
    fn traceparent(&self) -> &str {
        &self.traceparent
    }
    fn tracestate(&self) -> &str {
        &self.tracestate
    }
    fn baggage(&self) -> &str {
        &self.baggage
    }
    fn feature_variant(&self) -> &str {
        &self.feature_variant
    }
    fn properties(&self) -> &str {
        &self.properties
    }
    fn context(&self) -> &str {
        &self.context
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct Order {
    pub(crate) id: String,
    pub(crate) order_number: String,
    pub(crate) tenant_id: Option<String>,
    pub(crate) customer_id: Option<String>,
    pub(crate) status: Option<String>,
    pub(crate) payment_status: Option<String>,
    pub(crate) currency: Option<String>,
    pub(crate) subtotal_minor: Option<String>,
    pub(crate) discount_minor: Option<String>,
    pub(crate) tax_minor: Option<String>,
    pub(crate) shipping_minor: Option<String>,
    pub(crate) total_minor: Option<String>,
    pub(crate) created_at: Option<String>,
    pub(crate) items: Vec<OrderItem>,
}

#[graphql_object(context = StoreContext)]
impl Order {
    fn id(&self) -> &str {
        &self.id
    }
    fn order_number(&self) -> &str {
        &self.order_number
    }
    fn tenant_id(&self) -> Option<&str> {
        self.tenant_id.as_deref()
    }
    fn customer_id(&self) -> Option<&str> {
        self.customer_id.as_deref()
    }
    fn status(&self) -> Option<&str> {
        self.status.as_deref()
    }
    fn payment_status(&self) -> Option<&str> {
        self.payment_status.as_deref()
    }
    fn currency(&self) -> Option<&str> {
        self.currency.as_deref()
    }
    fn subtotal_minor(&self) -> Option<&str> {
        self.subtotal_minor.as_deref()
    }
    fn discount_minor(&self) -> Option<&str> {
        self.discount_minor.as_deref()
    }
    fn tax_minor(&self) -> Option<&str> {
        self.tax_minor.as_deref()
    }
    fn shipping_minor(&self) -> Option<&str> {
        self.shipping_minor.as_deref()
    }
    fn total_minor(&self) -> Option<&str> {
        self.total_minor.as_deref()
    }
    fn created_at(&self) -> Option<&str> {
        self.created_at.as_deref()
    }
    fn items(&self) -> &[OrderItem] {
        &self.items
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct OrderItem {
    pub(crate) sku: String,
    pub(crate) product_name: Option<String>,
    pub(crate) quantity: i32,
    pub(crate) unit_price_minor: Option<String>,
    pub(crate) discount_minor: Option<String>,
    pub(crate) line_total_minor: Option<String>,
}

#[graphql_object(context = StoreContext)]
impl OrderItem {
    fn sku(&self) -> &str {
        &self.sku
    }
    fn product_name(&self) -> Option<&str> {
        self.product_name.as_deref()
    }
    fn quantity(&self) -> i32 {
        self.quantity
    }
    fn unit_price_minor(&self) -> Option<&str> {
        self.unit_price_minor.as_deref()
    }
    fn discount_minor(&self) -> Option<&str> {
        self.discount_minor.as_deref()
    }
    fn line_total_minor(&self) -> Option<&str> {
        self.line_total_minor.as_deref()
    }
}

impl Order {
    pub(crate) fn try_from_value(value: &Value) -> anyhow::Result<Self> {
        let id = if value.get("id").is_some() {
            required_string_field(value, "id")?
        } else {
            required_string_field(value, "order_id")?
        };
        let items = match value.get("items") {
            None => Vec::new(),
            Some(Value::Array(values)) => values
                .iter()
                .map(OrderItem::try_from_value)
                .collect::<anyhow::Result<Vec<_>>>()?,
            Some(Value::Null) => return Err(anyhow!("order payload items are null")),
            Some(_) => return Err(anyhow!("order payload items are not an array")),
        };
        Ok(Self {
            id,
            order_number: required_string_field(value, "order_number")?,
            tenant_id: Some(required_string_field(value, "tenant_id")?),
            customer_id: Some(required_string_field(value, "customer_id")?),
            status: Some(required_string_field(value, "status")?),
            payment_status: optional_string_field(value, "payment_status")?,
            currency: Some(required_string_field(value, "currency")?),
            subtotal_minor: optional_numeric_string_field(value, "subtotal_minor")?,
            discount_minor: optional_numeric_string_field(value, "discount_minor")?,
            tax_minor: optional_numeric_string_field(value, "tax_minor")?,
            shipping_minor: optional_numeric_string_field(value, "shipping_minor")?,
            total_minor: optional_numeric_string_field(value, "total_minor")?,
            created_at: optional_timestamp_string_field(value, "created_at")?,
            items,
        })
    }
}

impl OrderItem {
    pub(crate) fn try_from_value(value: &Value) -> anyhow::Result<Self> {
        let quantity = required_i32_field(value, "quantity")?;
        if quantity <= 0 {
            return Err(anyhow!("order item quantity must be positive"));
        }
        Ok(Self {
            sku: required_string_field(value, "sku")?,
            product_name: optional_string_field(value, "product_name")?,
            quantity,
            unit_price_minor: Some(required_numeric_string_field(value, "unit_price_minor")?),
            discount_minor: Some(required_numeric_string_field(value, "discount_minor")?),
            line_total_minor: Some(required_numeric_string_field(value, "line_total_minor")?),
        })
    }
}

fn required_string_field(value: &Value, name: &str) -> anyhow::Result<String> {
    match value.get(name) {
        Some(Value::String(field)) if !field.trim().is_empty() => Ok(field.clone()),
        Some(Value::String(_)) => Err(anyhow!("{name} is empty")),
        Some(Value::Null) | None => Err(anyhow!("{name} is missing")),
        Some(_) => Err(anyhow!("{name} is not a string")),
    }
}

pub(crate) fn optional_string_field(value: &Value, name: &str) -> anyhow::Result<Option<String>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(field)) if !field.trim().is_empty() => Ok(Some(field.clone())),
        Some(Value::String(_)) => Err(anyhow!("{name} is empty")),
        Some(_) => Err(anyhow!("{name} is not a string")),
    }
}

fn required_numeric_string_field(value: &Value, name: &str) -> anyhow::Result<String> {
    optional_numeric_string_field(value, name)?.ok_or_else(|| anyhow!("{name} is missing"))
}

fn optional_numeric_string_field(value: &Value, name: &str) -> anyhow::Result<Option<String>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Number(number)) => {
            if let Some(number) = number.as_i64() {
                Ok(Some(number.to_string()))
            } else if let Some(number) = number.as_u64() {
                Ok(Some(number.to_string()))
            } else {
                Err(anyhow!("{name} is not an integer"))
            }
        }
        Some(Value::String(field)) => field
            .parse::<i64>()
            .map(|number| Some(number.to_string()))
            .map_err(|_| anyhow!("{name} is not an integer")),
        Some(_) => Err(anyhow!("{name} is not numeric")),
    }
}

fn optional_timestamp_string_field(value: &Value, name: &str) -> anyhow::Result<Option<String>> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(field)) if !field.trim().is_empty() => Ok(Some(field.clone())),
        Some(Value::Number(number)) if number.as_i64().is_some() || number.as_u64().is_some() => {
            Ok(Some(number.to_string()))
        }
        Some(Value::String(_)) => Err(anyhow!("{name} is empty")),
        Some(_) => Err(anyhow!("{name} is not a timestamp")),
    }
}

fn required_i32_field(value: &Value, name: &str) -> anyhow::Result<i32> {
    let Some(field) = value.get(name) else {
        return Err(anyhow!("{name} is missing"));
    };
    match field {
        Value::Number(number) => number
            .as_i64()
            .and_then(|number| i32::try_from(number).ok())
            .ok_or_else(|| anyhow!("{name} is not a 32-bit integer")),
        Value::String(field) => field
            .parse::<i32>()
            .map_err(|_| anyhow!("{name} is not a 32-bit integer")),
        _ => Err(anyhow!("{name} is not an integer")),
    }
}

pub(crate) fn u64_field(value: &Value, name: &str) -> Option<u64> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .or_else(|| value.get(name).and_then(Value::as_str)?.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::deterministic_event_id;

    #[test]
    fn analytics_event_id_is_stable_and_tenant_scoped() {
        let first = deterministic_event_id("tenant-acme", "browse:WIDGET-1");

        assert_eq!(
            first,
            deterministic_event_id("tenant-acme", "browse:WIDGET-1")
        );
        assert_ne!(
            first,
            deterministic_event_id("tenant-nova", "browse:WIDGET-1")
        );
        assert_eq!(
            uuid::Uuid::parse_str(&first)
                .expect("deterministic UUID")
                .get_version_num(),
            5
        );
    }
}
