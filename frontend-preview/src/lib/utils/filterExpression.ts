import type {
	FilterGroup,
	FilterOperator,
	FilterCondition,
} from '$types/dataBrowser';
import { t } from '$i18n';

// Identifier whitelist for property names. Anything that does not match is
// rejected so user input can never inject arbitrary query fragments into the
// generated WHERE expression.
const IDENTIFIER_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;

// Map UI operators to their query-expression equivalents. Value predicates are
// written with surrounding spaces so concatenation stays readable.
const OPERATOR_TEMPLATES: Record<FilterOperator, (field: string) => string> = {
	eq: (field) => `${field} ==`,
	ne: (field) => `${field} !=`,
	gt: (field) => `${field} >`,
	lt: (field) => `${field} <`,
	ge: (field) => `${field} >=`,
	le: (field) => `${field} <=`,
	contains: (field) => `${field} CONTAINS`,
	startsWith: (field) => `${field} STARTS WITH`,
	endsWith: (field) => `${field} ENDS WITH`,
};

// Strip control characters and escape backslashes and single quotes so a string
// literal cannot terminate early or smuggle in additional tokens.
export function escapeStringValue(value: string): string {
	// eslint-disable-next-line no-control-regex -- stripping control characters is the point of this sanitizer
	const cleaned = value.replace(/[\u0000-\u001f\u007f]/g, '');
	const escaped = cleaned.replace(/\\/g, '\\\\').replace(/'/g, "\\'");
	return `'${escaped}'`;
}

export function serializeValue(value: FilterCondition['value']): string {
	if (typeof value === 'number') {
		if (!Number.isFinite(value)) {
			throw new Error(t('errors.filterValueNotFinite'));
		}
		return String(value);
	}
	if (typeof value === 'boolean') {
		return value ? 'true' : 'false';
	}
	return escapeStringValue(String(value ?? ''));
}

export function isConditionValid(condition: FilterCondition): boolean {
	return (
		IDENTIFIER_PATTERN.test(condition.property) && condition.property.length > 0
	);
}

// Compile a filter group into a query WHERE fragment. The caller is expected to
// prepend the WHERE keyword; this only produces the predicate expression.
export function compileFilter(group: FilterGroup | null | undefined): string {
	if (
		!group ||
		!Array.isArray(group.conditions) ||
		group.conditions.length === 0
	) {
		return '';
	}
	const parts: string[] = [];
	for (const condition of group.conditions) {
		if (!isConditionValid(condition)) continue;
		const template = OPERATOR_TEMPLATES[condition.operator];
		if (!template) continue;
		let serialized: string;
		try {
			serialized = serializeValue(condition.value);
		} catch {
			continue;
		}
		parts.push(`${template(condition.property)} ${serialized}`);
	}
	if (parts.length === 0) return '';
	const joiner = group.logic === 'OR' ? ' OR ' : ' AND ';
	return parts.join(joiner);
}

export function isFilterValid(group: FilterGroup | null | undefined): boolean {
	if (!group || !group.conditions.length) return true;
	return group.conditions.every(isConditionValid);
}
