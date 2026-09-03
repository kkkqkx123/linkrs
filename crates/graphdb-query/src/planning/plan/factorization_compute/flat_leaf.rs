use crate::planning::plan::factorization::FactorizedSchema;

pub(super) fn flat_leaf() -> FactorizedSchema {
    let mut schema = FactorizedSchema::new();
    schema.create_flat_group(false);
    schema.validate_at_most_one_unflat();
    schema
}

pub(super) fn flatten_all_from_child(child_schemas: &[FactorizedSchema]) -> FactorizedSchema {
    let mut schema = child_schemas.first().cloned().unwrap_or_default();
    schema.flatten_all();
    schema.validate_at_most_one_unflat();
    schema
}
