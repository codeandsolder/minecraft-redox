package dev.minecraftredox.jimple;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import java.util.ArrayList;
import java.util.Comparator;
import java.util.IdentityHashMap;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import sootup.core.graph.ControlFlowGraph;
import sootup.core.inputlocation.AnalysisInputLocation;
import sootup.core.jimple.common.Value;
import sootup.core.jimple.common.stmt.Stmt;
import sootup.core.model.Body;
import sootup.core.model.SootClass;
import sootup.core.model.SootMethod;
import sootup.core.types.ClassType;
import sootup.core.views.View;
import sootup.java.bytecode.frontend.inputlocation.JavaClassPathAnalysisInputLocation;
import sootup.java.core.views.JavaView;

public final class DumpJson {
    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();
    private DumpJson() {}

    public static void main(String[] args) {
        if (args.length < 2 || args.length > 3) throw new IllegalArgumentException("usage: <class-path> <class-name> [method-name-substring]");
        AnalysisInputLocation input = new JavaClassPathAnalysisInputLocation(args[0]);
        View view = new JavaView(input);
        ClassType type = view.getIdentifierFactory().getClassType(args[1]);
        SootClass klass = view.getClass(type).orElseThrow();
        String filter = args.length == 3 ? args[2] : "";
        List<Object> methods = klass.getMethods().stream()
                .filter(m -> m.isConcrete() && m.getName().contains(filter))
                .sorted(Comparator.comparing(m -> m.getSignature().toString()))
                .map(DumpJson::method)
                .toList();
        System.out.println(GSON.toJson(methods));
    }

    private static Object method(SootMethod method) {
        Body body = method.getBody();
        List<Stmt> stmts = body.getStmts();
        Map<Stmt, Integer> ids = new IdentityHashMap<>();
        for (int i = 0; i < stmts.size(); i++) ids.put(stmts.get(i), i);
        ControlFlowGraph<?> cfg = body.getControlFlowGraph();
        List<Object> encoded = new ArrayList<>();
        for (int i = 0; i < stmts.size(); i++) {
            Stmt stmt = stmts.get(i);
            Map<String, Object> row = new LinkedHashMap<>();
            row.put("id", i);
            row.put("kind", stmt.getClass().getSimpleName());
            row.put("text", stmt.toString());
            row.put("def", stmt.getDef().map(Object::toString).orElse(null));
            row.put("uses", stmt.getUses().stream().map(DumpJson::value).toList());
            row.put("successors", cfg.successors(stmt).stream().map(ids::get).toList());
            encoded.add(row);
        }
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("signature", method.getSignature().toString());
        result.put("statements", encoded);
        return result;
    }

    private static Object value(Value value) {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("kind", value.getClass().getSimpleName());
        result.put("type", value.getType().toString());
        result.put("text", value.toString());
        return result;
    }
}
