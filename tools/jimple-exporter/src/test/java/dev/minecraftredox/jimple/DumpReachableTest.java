package dev.minecraftredox.jimple;

import com.google.gson.Gson;
import com.google.gson.JsonArray;
import java.net.URISyntaxException;
import java.nio.file.Path;
import java.util.HashSet;
import java.util.Set;
import org.junit.jupiter.api.Test;
import static org.junit.jupiter.api.Assertions.*;

class DumpReachableTest {
    private static final String BASE = DumpReachableTest.class.getName();

    interface Contract {
        int contractValue();
    }

    static class Parent {
        int virtualValue() { return 1; }
        int wrapper() { return virtualValue(); }
        int interfaceWrapper(Contract receiver) { return receiver.contractValue(); }
    }

    static class Unrelated {
        int virtualValue() { return 2; }
    }

    static class Root extends Parent implements Contract {
        @Override int virtualValue() { return 3; }
        @Override public int contractValue() { return 4; }
        int virtualEntry() { return wrapper(); }
        int interfaceEntry() { return interfaceWrapper(this); }
        int specialEntry() { return super.virtualValue(); }
        int unrelatedEntry(Unrelated other) { return other.virtualValue(); }
    }

    private static Set<String> signatures(String entry, int maxDepth) throws URISyntaxException {
        String fixturePath = Path.of(Root.class.getProtectionDomain().getCodeSource().getLocation().toURI()).toString();
        DumpReachable.Export export = DumpReachable.export(fixturePath, Root.class.getName(), entry, 32, maxDepth);
        JsonArray methods = new Gson().toJsonTree(export.methods()).getAsJsonArray();
        Set<String> signatures = new HashSet<>();
        methods.forEach(method -> signatures.add(method.getAsJsonObject().get("display_signature").getAsString()));
        return signatures;
    }

    private static String signature(String owner, String method) {
        return "<" + BASE + "$" + owner + ": int " + method + "()>";
    }

    @Test
    void includesOverrideReachedThroughInheritedWrapper() throws URISyntaxException {
        Set<String> methods = signatures("virtualEntry", 2);
        assertTrue(methods.contains(signature("Parent", "virtualValue")), methods.toString());
        assertTrue(methods.contains(signature("Root", "virtualValue")), methods.toString());
        assertFalse(signatures("virtualEntry", 1).contains(signature("Root", "virtualValue")));
    }

    @Test
    void includesInterfaceImplementation() throws URISyntaxException {
        Set<String> methods = signatures("interfaceEntry", 2);
        assertTrue(methods.contains(signature("Root", "contractValue")), methods.toString());
    }

    @Test
    void doesNotInventDispatchForSpecialOrUnrelatedCalls() throws URISyntaxException {
        Set<String> special = signatures("specialEntry", 2);
        assertTrue(special.contains(signature("Parent", "virtualValue")), special.toString());
        assertFalse(special.contains(signature("Root", "virtualValue")), special.toString());
        Set<String> unrelated = signatures("unrelatedEntry", 2);
        assertTrue(unrelated.contains(signature("Unrelated", "virtualValue")), unrelated.toString());
        assertFalse(unrelated.contains(signature("Root", "virtualValue")), unrelated.toString());
    }
}
