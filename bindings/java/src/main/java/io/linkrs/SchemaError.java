package io.linkrs;

/** Primary status code 16 (see {@link LinkrsError}). */
public class SchemaError extends LinkrsError {
    public SchemaError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
