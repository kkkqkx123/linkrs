package io.linkrs;

/** Primary status code 19 (see {@link LinkrsError}). */
public class TypeMismatchError extends LinkrsError {
    public TypeMismatchError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
