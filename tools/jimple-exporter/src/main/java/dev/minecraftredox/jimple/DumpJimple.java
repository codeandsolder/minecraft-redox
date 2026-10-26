package dev.minecraftredox.jimple;

import java.util.Comparator;
import sootup.core.inputlocation.AnalysisInputLocation;
import sootup.core.model.SootClass;
import sootup.core.model.SootMethod;
import sootup.core.types.ClassType;
import sootup.core.views.View;
import sootup.java.bytecode.frontend.inputlocation.JavaClassPathAnalysisInputLocation;
import sootup.java.core.views.JavaView;

public final class DumpJimple {
    private DumpJimple() {}

    public static void main(String[] args) {
        if (args.length < 2 || args.length > 3) {
            throw new IllegalArgumentException("usage: <class-path> <class-name> [method-name-substring]");
        }
        String classPath = args[0];
        String className = args[1];
        String methodFilter = args.length == 3 ? args[2] : "";

        AnalysisInputLocation input = new JavaClassPathAnalysisInputLocation(classPath);
        View view = new JavaView(input);
        ClassType type = view.getIdentifierFactory().getClassType(className);
        SootClass klass = view.getClass(type).orElseThrow(() -> new IllegalArgumentException("class not found: " + className));

        klass.getMethods().stream()
                .filter(method -> method.getName().contains(methodFilter))
                .sorted(Comparator.comparing(method -> method.getSignature().toString()))
                .forEach(DumpJimple::printMethod);
    }

    private static void printMethod(SootMethod method) {
        System.out.println("=== " + method.getSignature() + " ===");
        if (!method.isConcrete()) {
            System.out.println("<non-concrete>");
            return;
        }
        System.out.println(method.getBody());
    }
}
