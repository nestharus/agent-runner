//! ## Declared roles
//!
//! - accessor
//! - formatter
//! - mapper
//! - parser
//! - orchestration
//!
//! Role set: { accessor, formatter, mapper, parser, orchestration }
//!
//! Discovered model parameter persistence methods for `StateDb`.

use super::*;

struct ModelParameterJson {
    param_type_json: String,
    cli_mapping_json: String,
}

struct ModelParameterRawRow {
    name: String,
    display_name: String,
    param_type: String,
    description: String,
    cli_mapping: String,
}

struct ModelParameterParsedFields {
    param_type: ParamType,
    cli_mapping: CliMapping,
}

impl StateDb {
    /// Insert or update a model parameter.
    pub fn upsert_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        param: &ModelParameter,
    ) -> Result<(), String> {
        let json = Self::format_model_parameter_json(param)?;
        self.write_model_parameter(model_name, provider, param, &json)
    }

    fn format_model_parameter_json(param: &ModelParameter) -> Result<ModelParameterJson, String> {
        Ok(ModelParameterJson {
            param_type_json: Self::serialize_param_type(&param.param_type)?,
            cli_mapping_json: Self::serialize_cli_mapping(&param.cli_mapping)?,
        })
    }

    fn write_model_parameter(
        &self,
        model_name: &str,
        provider: &str,
        param: &ModelParameter,
        json: &ModelParameterJson,
    ) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO model_parameters (model_name, provider, name, display_name, param_type, description, cli_mapping)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT (model_name, provider, name)
                 DO UPDATE SET
                    display_name = ?4,
                    param_type = ?5,
                    description = ?6,
                    cli_mapping = ?7",
                sqlite::params![
                    model_name,
                    provider,
                    &param.name,
                    &param.display_name,
                    &json.param_type_json,
                    &param.description,
                    &json.cli_mapping_json,
                ],
            )
            .map_err(|e| format!("Failed to upsert model parameter: {e}"))?;
        Ok(())
    }

    fn serialize_param_type(param_type: &ParamType) -> Result<String, String> {
        serde_json::to_string(param_type)
            .map_err(|e| format!("Failed to serialize param_type: {e}"))
    }

    fn serialize_cli_mapping(cli_mapping: &CliMapping) -> Result<String, String> {
        serde_json::to_string(cli_mapping)
            .map_err(|e| format!("Failed to serialize cli_mapping: {e}"))
    }

    /// List all parameters for a given model and provider.
    pub fn list_model_parameters(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameter>, String> {
        let rows = self.query_model_parameter_rows(model_name, provider)?;
        rows.into_iter()
            .map(Self::parse_model_parameter_raw_row)
            .collect()
    }

    fn query_model_parameter_rows(
        &self,
        model_name: &str,
        provider: &str,
    ) -> Result<Vec<ModelParameterRawRow>, String> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT name, display_name, param_type, description, cli_mapping
                 FROM model_parameters
                 WHERE model_name = ?1 AND provider = ?2
                 ORDER BY name",
            )
            .map_err(|e| format!("Failed to prepare query: {e}"))?;

        let rows = stmt
            .query_map(
                sqlite::params![model_name, provider],
                Self::model_parameter_row_mapper,
            )
            .map_err(|e| format!("Failed to query model parameters: {e}"))?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row.map_err(|e| format!("Failed to read parameter row: {e}"))?);
        }
        Ok(result)
    }

    fn model_parameter_row_mapper(row: &sqlite::Row<'_>) -> sqlite::Result<ModelParameterRawRow> {
        Ok(ModelParameterRawRow {
            name: row.get(0)?,
            display_name: row.get(1)?,
            param_type: row.get(2)?,
            description: row.get(3)?,
            cli_mapping: row.get(4)?,
        })
    }

    fn parse_model_parameter_raw_row(raw: ModelParameterRawRow) -> Result<ModelParameter, String> {
        let parsed = Self::parse_model_parameter_serialized_fields(&raw)?;
        Ok(ModelParameter {
            name: raw.name,
            display_name: raw.display_name,
            param_type: parsed.param_type,
            description: raw.description,
            cli_mapping: parsed.cli_mapping,
        })
    }

    fn parse_model_parameter_serialized_fields(
        raw: &ModelParameterRawRow,
    ) -> Result<ModelParameterParsedFields, String> {
        Ok(ModelParameterParsedFields {
            param_type: Self::parse_param_type_json(&raw.param_type)?,
            cli_mapping: Self::parse_cli_mapping_json(&raw.cli_mapping)?,
        })
    }

    fn parse_param_type_json(raw: &str) -> Result<ParamType, String> {
        serde_json::from_str(raw).map_err(|e| format!("Failed to deserialize param_type: {e}"))
    }

    fn parse_cli_mapping_json(raw: &str) -> Result<CliMapping, String> {
        serde_json::from_str(raw).map_err(|e| format!("Failed to deserialize cli_mapping: {e}"))
    }
}
