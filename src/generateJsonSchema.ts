import { FoxgloveMessageSchema, FoxglovePrimitive } from "./types";

function primitiveToJsonSchema(type: Exclude<FoxglovePrimitive, "bytes">) {
  switch (type) {
    case "string":
      return { type: "string" };
    case "boolean":
      return { type: "boolean" };
    case "float64":
      return { type: "number" };
    case "uint32":
      return { type: "integer", minimum: 0 };
    case "time":
      return {
        type: "object",
        title: "time",
        properties: {
          sec: { type: "integer", minimum: 0 },
          nsec: { type: "integer", minimum: 0, maximum: 999_999_999 },
        },
      };
    case "duration":
      return {
        type: "object",
        title: "duration",
        properties: {
          sec: { type: "integer" },
          nsec: { type: "integer", minimum: 0, maximum: 999_999_999 },
        },
      };
  }
}

export function generateJsonSchema(schema: FoxgloveMessageSchema): Record<string, unknown> {
  const properties: Record<string, unknown> = {};
  for (const field of schema.fields) {
    let fieldType: Record<string, unknown>;
    switch (field.type.type) {
      case "primitive":
        if (field.type.name === "bytes") {
          fieldType = { type: "string", contentEncoding: "base64" };
        } else {
          fieldType = primitiveToJsonSchema(field.type.name);
        }
        break;
      case "nested":
        fieldType = generateJsonSchema(field.type.schema);
        break;
      case "enum":
        fieldType = {
          title: `${field.type.enum.name}: ${field.type.enum.description}`,
          description: field.description,
          oneOf: field.type.enum.values.map(({ name, value, description }) => ({
            title: name,
            const: value,
            description,
          })),
        };
        break;
    }
    if (typeof field.array === "number") {
      fieldType = {
        type: "array",
        items: fieldType,
        minItems: field.array,
        maxItems: field.array,
      };
    } else if (field.array === true) {
      fieldType = { type: "array", items: fieldType };
    }
    fieldType.description = field.description;
    properties[field.name] = fieldType;
  }

  return {
    $comment: `Generated from ${schema.name} by @foxglove/schemas`,
    title: schema.name,
    description: schema.description,
    type: "object",
    properties,
  };
}