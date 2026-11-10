package io.linkrs;

/** Primary status code 18 (see {@link LinkrsError}). */
public class ConstraintError extends LinkrsError {
    public ConstraintError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
