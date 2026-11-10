package io.linkrs;

/** Primary status code 12 (see {@link LinkrsError}). */
public class NotFoundError extends LinkrsError {
    public NotFoundError(String message, int code, int extended, Integer offset) {
        super(message, code, extended, offset);
    }
}
