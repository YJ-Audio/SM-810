use crate::{Engine, Error, Result};
use sampler_db as db;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Query {
	All { conditions: Vec<Query> },
	Any { conditions: Vec<Query> },
	Not { condition: Box<Query> },
	Tag { name: String },
	Text { text: String },
	Root { id: i64 },
	Field { field: Field, op: Comparison, value: f64 },
	SimilarTo { id: i64, count: usize },
	Semantic { text: String, count: usize },
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Field {
	DurationMs,
	Bpm,
	KeyRoot,
	Lufs,
	IsLoop,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Comparison {
	Eq,
	Ne,
	Lt,
	Le,
	Gt,
	Ge,
}
impl Default for Query {
	fn default() -> Self {
		Self::All { conditions: Vec::new() }
	}
}
impl Engine {
	pub fn query_ids(&self, query: &Query) -> Result<HashSet<i64>> {
		let universe: HashSet<_> = db::matching_ids(&self.reader()?, &db::BrowseQuery::default())?
			.into_iter()
			.collect();
		self.resolve_query(query, &universe, 0)
	}
	fn resolve_query(&self, query: &Query, universe: &HashSet<i64>, depth: usize) -> Result<HashSet<i64>> {
		if depth > 32 {
			return Err(Error::Invalid("Query nesting exceeds 32 levels".into()));
		}
		let request = match query {
			Query::All { conditions } => {
				let mut result = universe.clone();
				for condition in conditions {
					let matches = self.resolve_query(condition, universe, depth + 1)?;
					result.retain(|id| matches.contains(id));
				}
				return Ok(result);
			}
			Query::Any { conditions } => {
				let mut result = HashSet::new();
				for condition in conditions {
					result.extend(self.resolve_query(condition, universe, depth + 1)?);
				}
				return Ok(result);
			}
			Query::Not { condition } => {
				let excluded = self.resolve_query(condition, universe, depth + 1)?;
				return Ok(universe.difference(&excluded).copied().collect());
			}
			Query::Field { field, op, value } => {
				if !value.is_finite() {
					return Err(Error::Invalid("Field comparison requires a finite value".into()));
				}
				let field = match field {
					Field::DurationMs => "duration_ms",
					Field::Bpm => "bpm",
					Field::KeyRoot => "key_root",
					Field::Lufs => "lufs",
					Field::IsLoop => "is_loop",
				};
				let op = match op {
					Comparison::Eq => "=",
					Comparison::Ne => "!=",
					Comparison::Lt => "<",
					Comparison::Le => "<=",
					Comparison::Gt => ">",
					Comparison::Ge => ">=",
				};
				let reader = self.reader()?;
				let mut statement = reader
					.prepare(&format!("SELECT sample_id FROM analysis WHERE {field} {op} ?1"))
					.map_err(db::Error::from)?;
				let ids = statement
					.query_map([value], |row| row.get::<_, i64>(0))
					.map_err(db::Error::from)?;
				return ids
					.map(|id| id.map_err(db::Error::from).map_err(Error::from))
					.filter(|id| id.as_ref().map_or(true, |id| universe.contains(id)))
					.collect();
			}
			Query::SimilarTo { id, count } => {
				let vector = self.embed_one(*id)?;
				return self.query_vector(&vector, *count, universe);
			}
			Query::Semantic { text, count } => {
				let vector = self.with_model(|model| Ok(model.text(text)?))?;
				return self.query_vector(&vector, *count, universe);
			}
			Query::Tag { name } => db::BrowseQuery {
				tag: Some(name.clone()),
				..Default::default()
			},
			Query::Text { text } => db::BrowseQuery {
				text: text.clone(),
				..Default::default()
			},
			Query::Root { id } => db::BrowseQuery {
				root_id: Some(*id),
				..Default::default()
			},
		};
		Ok(db::matching_ids(&self.reader()?, &request)?.into_iter().collect())
	}
	fn query_vector(&self, vector: &[f32], count: usize, universe: &HashSet<i64>) -> Result<HashSet<i64>> {
		Ok(self
			.vectors
			.read()
			.map_err(|_| Error::Invalid("Vector store lock poisoned".into()))?
			.search(vector, count.min(100000), Some(universe), None)?
			.into_iter()
			.map(|hit| hit.id)
			.collect())
	}
}
