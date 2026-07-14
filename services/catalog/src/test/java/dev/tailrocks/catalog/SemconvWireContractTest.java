package dev.tailrocks.catalog;

import com.fasterxml.jackson.databind.JsonNode;
import com.fasterxml.jackson.databind.ObjectMapper;
import io.tailrocks.semconv.Semconv;
import java.lang.reflect.Field;
import java.nio.file.Path;
import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;

class SemconvWireContractTest {
    private static final ObjectMapper JSON = new ObjectMapper();

    @Test
    void generated_java_constants_match_the_cross_language_wire_fixture() throws Exception {
        JsonNode constants = JSON.readTree(
            Path.of("../../fixtures/semconv-wire-contract.json").toFile()
        ).required("constants");

        for (JsonNode constant : constants) {
            Field field = Semconv.class.getField(constant.required("java").asText());
            Object actual = field.get(null);
            JsonNode scalar = constant.get("value");
            if (!scalar.isNull()) {
                assertEquals(scalar.asText(), actual, constant.required("id").asText());
                continue;
            }

            JsonNode values = constant.required("values");
            String[] expected = new String[values.size()];
            for (int index = 0; index < values.size(); index++) {
                expected[index] = values.get(index).asText();
            }
            assertArrayEquals(expected, (String[]) actual, constant.required("id").asText());
        }
    }
}
