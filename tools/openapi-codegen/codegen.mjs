import { readFileSync, writeFileSync } from 'fs';

const ROOT_DIR = '/home/kkkqkx/code/linkrs';
const OPENAPI_PATH = `${ROOT_DIR}/frontend/openapi.json`;
const OUTPUT_PATH = `${ROOT_DIR}/frontend/src/lib/types/schema.d.ts`;

async function main() {
  const openapiJson = JSON.parse(readFileSync(OPENAPI_PATH, 'utf-8'));
  
  const parsed = openapiJson;
  
  console.log(`Found ${Object.keys(parsed.components.schemas || {}).length} schemas`);
  console.log(`Found ${Object.keys(parsed.paths || {}).length} paths`);
  
  const tsTypes = generateSchemaTypes(parsed.components.schemas || {});
  writeFileSync(OUTPUT_PATH, tsTypes, 'utf-8');
  
  console.log(`Generated ${OUTPUT_PATH}`);
}

function generateSchemaTypes(schemas) {
  const lines = [
    '// Auto-generated TypeScript types from OpenAPI schema',
    '// DO NOT EDIT - Run npm run generate in frontend/codegen to update',
    '',
  ];
  
  const typeMap = new Map();
  for (const [name, schema] of Object.entries(schemas)) {
    const typeName = name;
    const typeDef = convertToTypeScript(schema, typeName);
    if (typeDef) {
      typeMap.set(name, typeDef);
    }
  }
  
  // Generate type definitions
  let first = true;
  for (const [typeName, typeDef] of typeMap.entries()) {
    if (!first) {
      lines.push('');
    }
    first = false;
    lines.push(typeDef);
  }
  
  return lines.join('\n') + '\n';
}

// Top-level conversion - returns exported type/interface declarations
function convertToTypeScript(schema, typeName) {
  const schemaType = schema.type;
  
  // Handle anyOf/oneOf/allOf at top level
  if (!schemaType && schema.anyOf) {
    return `export type ${typeName} = ${convertUnionTypeExpression(schema.anyOf)};`;
  }
  
  if (!schemaType && schema.oneOf) {
    return `export type ${typeName} = ${convertUnionTypeExpression(schema.oneOf)};`;
  }
  
  if (!schemaType && schema.allOf) {
    return `export type ${typeName} = ${convertIntersectionTypeExpression(schema.allOf)};`;
  }
  
  switch (schemaType) {
    case 'object':
      return convertObjectType(schema, typeName);
    case 'array':
      return `export type ${typeName} = ${convertArrayTypeExpression(schema.items)}[];`;
    case 'string':
      return `export type ${typeName} = string;`;
    case 'integer':
    case 'number':
      return `export type ${typeName} = number;`;
    case 'boolean':
      return `export type ${typeName} = boolean;`;
    default:
      return undefined;
  }
}

// Returns just a type expression (not a declaration)
function convertUnionTypeExpression(options) {
  return options.map(opt => convertInlineType(opt)).filter(Boolean).join(' | ');
}

function convertIntersectionTypeExpression(allOfSchemas) {
  return allOfSchemas.map(schema => {
    const ref = schema.$ref;
    if (ref) {
      return getRefName(ref);
    }
    return 'unknown';
  }).join(' & ');
}

// Convert array items type
function convertArrayTypeExpression(items) {
  if (!items) return 'unknown';
  return convertInlineType(items);
}

function convertObjectType(schema, typeName) {
  const required = schema.required || [];
  const properties = schema.properties || {};
  
  const lines = ['export interface ' + typeName + ' {'];
  
  for (const [propName, propSchema] of Object.entries(properties)) {
    const isRequired = required.includes(propName);
    const optional = isRequired ? '' : '?';
    const type = convertInlineType(propSchema);
    
    if (type === undefined || type === 'unknown' || !type) {
      // Skip fields with unknown/undefined types
      continue;
    }
    
    lines.push(`  ${propName}${optional}: ${type};`);
  }
  
  lines.push('}');
  
  return lines.join('\n');
}

// Convert inline type - handles refs, primitives, inline objects, unions
function convertInlineType(schema) {
  const ref = schema.$ref;
  if (ref) {
    return getRefName(ref);
  }
  
  const schemaType = schema.type;
  
  // Handle anyOf/oneOf/allOf at this level - these are inline types
  if (!schemaType && schema.anyOf) {
    return convertUnionTypeExpression(schema.anyOf);
  }
  if (!schemaType && schema.oneOf) {
    return convertUnionTypeExpression(schema.oneOf);
  }
  if (!schemaType && schema.allOf) {
    return convertIntersectionTypeExpression(schema.allOf);
  }
  
  // Handle null type or empty object
  if (schemaType === 'null') {
    return 'null';
  }
  
  if (!schemaType || (!schema.enum && !schema.properties && !schema.items)) {
    return 'unknown';
  }
  
  switch (schemaType) {
    case 'string':
      if (schema.enum) {
        return formatEnumType(schema.enum);
      }
      return 'string';
    case 'integer':
    case 'number':
      return 'number';
    case 'boolean':
      return 'boolean';
    case 'array':
      const itemType = convertInlineType(schema.items);
      return `${itemType}[]`;
    case 'object': {
      const props = schema.properties || {};
      const required = schema.required || [];
      
      if (Object.keys(props).length === 0) {
        return '{}';
      }
      
      const lines = ['{'];
      for (const [key, val] of Object.entries(props)) {
        const opt = required.includes(key) ? '' : '?';
        const t = convertInlineType(val);
        if (t !== undefined && t !== 'unknown' && t) {
          lines.push(`  ${key}${opt}: ${t};`);
        }
      }
      lines.push('}');
      return lines.join('\n');
    }
    default:
      return 'unknown';
  }
}

function formatEnumType(values) {
  return values.map(v => 
    typeof v === 'string' ? `'${v}'` : String(v)
  ).join(' | ');
}

function getRefName(ref) {
  const parts = ref.split('/');
  return parts[parts.length - 1];
}

main().catch(err => {
  console.error('Failed to generate:', err);
  process.exit(1);
});
