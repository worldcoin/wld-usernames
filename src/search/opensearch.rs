use anyhow::{Context, Result};
use aws_config::meta::region::RegionProviderChain;

use opensearch::{
	cert::CertificateValidation,
	http::{
		transport::{SingleNodeConnectionPool, TransportBuilder},
		StatusCode,
	},
	indices::{IndicesCreateParts, IndicesExistsParts, IndicesPutTemplateParts},
	OpenSearch, SearchParts,
};
use serde_json::{json, Value};
use std::{env, time::Duration};
use tracing::{error, info};
use url::Url;

use crate::types::{Address, UsernameRecord};

const DEFAULT_INDEX_NAME: &str = "names";
const DEFAULT_ENDPOINT: &str = "http://localhost:9200";

/// client for interacting with `OpenSearch`
pub struct OpenSearchClient {
	client: OpenSearch,
	index_name: String,
}

impl OpenSearchClient {
	pub async fn new() -> Result<Self> {
		let opensearch_url =
			env::var("OPENSEARCH_ENDPOINT").unwrap_or_else(|_| DEFAULT_ENDPOINT.to_string());
		let index_name =
			env::var("OPENSEARCH_INDEX_NAME").unwrap_or_else(|_| DEFAULT_INDEX_NAME.to_string());

		info!("Connecting to OpenSearch at {}", opensearch_url);

		let base_url = Url::parse(&opensearch_url).context("Failed to parse OpenSearch URL")?;
		let conn_pool = SingleNodeConnectionPool::new(base_url.clone().into());

		let transport = if base_url.host_str() == Some("localhost") {
			TransportBuilder::new(conn_pool)
				.timeout(Duration::from_secs(30))
				.cert_validation(CertificateValidation::None)
				.disable_proxy()
				.build()?
		} else {
			let region_provider = RegionProviderChain::default_provider().or_else("us-east-1");
			let aws_config = aws_config::from_env().region(region_provider).load().await;
			TransportBuilder::new(conn_pool)
				.timeout(Duration::from_secs(30))
				.auth(aws_config.try_into()?)
				.service_name("es")
				.build()?
		};

		let client = OpenSearch::new(transport);

		let opensearch_client = Self { client, index_name };
		opensearch_client.ensure_index_exists().await?;

		Ok(opensearch_client)
	}

	async fn ensure_index_exists(&self) -> Result<()> {
		// check if index exists
		let response = self
			.client
			.indices()
			.exists(IndicesExistsParts::Index(&[&self.index_name]))
			.send()
			.await?;

		if response.status_code() == StatusCode::NOT_FOUND {
			info!("Index {} doesn't exist - creating it", self.index_name);
			// index template for dms
			let template = json!({
				"index_patterns": [&self.index_name],
				"template": {
					"settings": {
						"index": {
							"number_of_shards": 1,
							"number_of_replicas": 1
						},
						"analysis": {
							"analyzer": {
								"username_analyzer": {
									"type": "custom",
									"tokenizer": "standard",
									"filter": ["lowercase", "asciifolding"]
								}
							}
						}
					},
					"mappings": {
						"properties": {
							"username": {
								"type": "text",
								"analyzer": "username_analyzer",
								"fields": {
									"keyword": {
										"type": "keyword"
									},
									"completion": {
										"type": "completion"
									}
								}
							},
							"address": {
								"type": "keyword"
							},
							"profile_picture_url": {
								"type": "keyword"
							}
						}
					}
				}
			});

			let response = self
				.client
				.indices()
				.put_template(IndicesPutTemplateParts::Name("username_template"))
				.body(template)
				.send()
				.await?;

			if !response.status_code().is_success() {
				let error_text = response.text().await?;
				error!("Failed to create index template: {}", error_text);
				return Err(anyhow::anyhow!(
					"Failed to create index template: {}",
					error_text
				));
			}

			let response = self
				.client
				.indices()
				.create(IndicesCreateParts::Index(&self.index_name))
				.send()
				.await?;

			if !response.status_code().is_success() {
				let error_text = response.text().await?;
				error!("Failed to create index: {}", error_text);
				return Err(anyhow::anyhow!("Failed to create index: {}", error_text));
			}
		} else {
			info!("Index {} already exists", self.index_name);
		}

		Ok(())
	}

	/// search for usernames with fuzzy matching
	pub async fn search_usernames(&self, query: &str, limit: usize) -> Result<Vec<UsernameRecord>> {
		let search_query = json!({
			"size": limit,
			"query": {
				"bool": {
					"should": [
						{
							"match": {
								"username": {
									"query": query,
									"fuzziness": "AUTO",
									"prefix_length": 1
								}
							}
						},
						{
							"prefix": {
								"username": {
									"value": query,
									"boost": 2.0
								}
							}
						}
					]
				}
			},
			"sort": [
				"_score"
			]
		});

		let response = self
			.client
			.search(SearchParts::Index(&[&self.index_name]))
			.body(search_query)
			.send()
			.await?;

		if !response.status_code().is_success() {
			let error_text = response.text().await?;
			error!("Search failed: {}", error_text);
			return Err(anyhow::anyhow!("Search failed: {}", error_text));
		}

		let response_body = response.json::<Value>().await?;
		let hits = response_body["hits"]["hits"]
			.as_array()
			.context("Expected hits array in response")?;

		let mut results = Vec::with_capacity(hits.len());

		for hit in hits {
			let source = hit["_source"]
				.as_object()
				.context("Expected _source object")?;

			let username = source["username"]
				.as_str()
				.context("Expected username string")?
				.to_string();

			let address_str = source["address"]
				.as_str()
				.context("Expected address string")?
				.to_string();

			let address = Address::from_string(&address_str)
				.context("Failed to parse address from string")?;

			let profile_picture_url = source["profile_picture_url"]
				.as_str()
				.and_then(|url| url.parse().ok());

			let record = UsernameRecord {
				username,
				address,
				profile_picture_url,
			};

			results.push(record);
		}

		Ok(results)
	}
}
