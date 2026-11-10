package io.linkrs;

import com.fasterxml.jackson.core.JsonProcessingException;
import com.fasterxml.jackson.core.type.TypeReference;
import com.fasterxml.jackson.databind.ObjectMapper;
import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;

/** Shared JSON and error-mapping helpers over the JNI boundary. */
final class Json {
    private static final ObjectMapper MAPPER = new ObjectMapper();
    private static final TypeReference<List<String>> STRING_LIST = new TypeReference<>() {
    };
    private static final TypeReference<List<Map<String, Object>>> ROW_LIST = new TypeReference<>() {
    };
    private static final TypeReference<Map<String, Object>> ROW = new TypeReference<>() {
    };

    private Json() {
    }

    static byte[] encode(Object value) {
        if (value == null) {
            return null;
        }
        try {
            return MAPPER.writeValueAsBytes(value);
        } catch (JsonProcessingException e) {
            throw new MisuseError("value is not JSON serializable: " + e.getMessage(), 20, 0, null);
        }
    }

    static List<String> stringList(byte[] json) {
        return parse(json, STRING_LIST);
    }

    static List<Map<String, Object>> rowList(byte[] json) {
        return parse(json, ROW_LIST);
    }

    private static final TypeReference<Map<String, Object>> ENVELOPE = new TypeReference<>() {
    };

    /** Unwrap the engine {columns, rows, metadata} envelope to its row list. */
    static List<Map<String, Object>> envelopeRows(byte[] json) {
        Map<String, Object> envelope = parse(json, ENVELOPE);
        Object rows = envelope.get("rows");
        if (rows instanceof List<?> list) {
            @SuppressWarnings("unchecked")
            List<Map<String, Object>> typed = (List<Map<String, Object>>) (List<?>) list;
            return typed;
        }
        return rowList(json);
    }

    static Map<String, Object> row(byte[] json) {
        return parse(json, ROW);
    }

    private static <T> T parse(byte[] json, TypeReference<T> type) {
        try {
            return MAPPER.readValue(json, type);
        } catch (Exception e) {
            throw new LinkrsError("failed to decode engine response: " + e.getMessage(), 2, 0, null);
        }
    }

    static String text(byte[] utf8) {
        return utf8 == null ? null : new String(utf8, StandardCharsets.UTF_8);
    }

    static LinkrsError failure(int code, long sessionHandle) {
        String message = text(NativeLib.nLastErrorText());
        int extended = sessionHandle == 0 ? 0 : NativeLib.nExtendedErrcode(sessionHandle);
        int rawOffset = sessionHandle == 0 ? -1 : NativeLib.nErrorOffset(sessionHandle);
        Integer offset = rawOffset >= 0 ? rawOffset : null;
        return switch (code) {
            case 5, 6 -> new BusyError(message, code, extended, offset);
            case 8 -> new ReadonlyError(message, code, extended, offset);
            case 12 -> new NotFoundError(message, code, extended, offset);
            case 16 -> new SchemaError(message, code, extended, offset);
            case 18 -> new ConstraintError(message, code, extended, offset);
            case 19 -> new TypeMismatchError(message, code, extended, offset);
            case 20 -> new MisuseError(message, code, extended, offset);
            default -> new LinkrsError(message, code, extended, offset);
        };
    }

    static void check(int code, long sessionHandle) {
        if (code != 0) {
            throw failure(code, sessionHandle);
        }
    }
}
