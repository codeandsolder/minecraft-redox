package dev.minecraftredox.jimple;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;
import java.util.ArrayDeque;
import java.util.Comparator;
import java.util.HashSet;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;
import java.util.Optional;
import java.util.Set;
import sootup.core.inputlocation.AnalysisInputLocation;
import sootup.core.jimple.common.Value;
import sootup.core.jimple.common.expr.AbstractInstanceInvokeExpr;
import sootup.core.jimple.common.expr.AbstractInvokeExpr;
import sootup.core.jimple.common.expr.JDynamicInvokeExpr;
import sootup.core.jimple.common.expr.JInterfaceInvokeExpr;
import sootup.core.jimple.common.expr.JVirtualInvokeExpr;
import sootup.core.jimple.common.expr.JNewExpr;
import sootup.core.jimple.common.ref.JStaticFieldRef;
import sootup.core.jimple.common.stmt.JAssignStmt;
import sootup.core.jimple.common.stmt.JIdentityStmt;
import sootup.core.jimple.common.stmt.JIfStmt;
import sootup.core.jimple.common.stmt.JInvokeStmt;
import sootup.core.jimple.common.stmt.JReturnStmt;
import sootup.core.jimple.common.stmt.JThrowStmt;
import sootup.core.jimple.common.stmt.Stmt;
import sootup.core.jimple.javabytecode.stmt.JSwitchStmt;
import sootup.core.model.SootClass;
import sootup.core.model.SootMethod;
import sootup.core.signatures.MethodSignature;
import sootup.core.types.ClassType;
import sootup.core.views.View;
import sootup.java.bytecode.frontend.inputlocation.JavaClassPathAnalysisInputLocation;
import sootup.java.core.views.JavaView;

/**
 * Emit a bounded transitive closure of concrete methods reachable from one root method.
 *
 * <p>The closure deliberately stops at runtime-library boundaries that minecraft-redox models
 * directly. Everything else is discovered from bytecode references, not from project-specific
 * method names. Both method count and call depth are bounded because an eager static call graph
 * otherwise includes large branches that abstract execution never reaches.
 */
public final class DumpReachable {
    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();
    private static final int DEFAULT_MAX_METHODS = 2_000;
    private static final int DEFAULT_MAX_DEPTH = 8;

    private record Pending(MethodSignature signature, int depth) {}

    private final View view;
    private final int maxMethods;
    private final int maxDepth;
    private final ClassType rootRuntimeType;
    private final Set<ClassType> rootAncestors;
    private final ArrayDeque<Pending> pending = new ArrayDeque<>();
    private final Map<String, SootMethod> methods = new LinkedHashMap<>();
    private final Set<String> queued = new HashSet<>();
    private final Set<String> initializerOwners = new HashSet<>();

    private DumpReachable(View view, ClassType rootRuntimeType, int maxMethods, int maxDepth) {
        this.view = view;
        this.rootRuntimeType = rootRuntimeType;
        this.rootAncestors = ancestorsOf(rootRuntimeType);
        this.maxMethods = maxMethods;
        this.maxDepth = maxDepth;
    }

    public static void main(String[] args) {
        if (args.length < 3 || args.length > 5) {
            throw new IllegalArgumentException(
                    "usage: <class-path> <root-class> <method-name-substring> [max-methods] [max-depth]");
        }
        int maxMethods = args.length >= 4 ? Integer.parseInt(args[3]) : DEFAULT_MAX_METHODS;
        int maxDepth = args.length == 5 ? Integer.parseInt(args[4]) : DEFAULT_MAX_DEPTH;
        Export export = export(args[0], args[1], args[2], maxMethods, maxDepth);
        System.err.printf(
                "reachable Jimple: %d methods across %d classes (max depth %d)%n",
                export.methods().size(),
                export.classes(),
                maxDepth);
        System.out.println(GSON.toJson(export.methods()));
    }

    record Export(List<Object> methods, long classes) {}

    static Export export(String classPath, String rootClassName, String methodSubstring,
                         int maxMethods, int maxDepth) {
        if (maxMethods <= 0 || maxDepth < 0) {
            throw new IllegalArgumentException("max-methods must be positive and max-depth non-negative");
        }

        AnalysisInputLocation input = new JavaClassPathAnalysisInputLocation(classPath);
        View view = new JavaView(input);
        ClassType rootType = view.getIdentifierFactory().getClassType(rootClassName);
        SootClass rootClass = view.getClass(rootType)
                .orElseThrow(() -> new IllegalArgumentException("class not found: " + rootClassName));
        List<? extends SootMethod> roots = rootClass.getMethods().stream()
                .filter(SootMethod::isConcrete)
                .filter(method -> method.getName().contains(methodSubstring))
                .sorted(Comparator.comparing(method -> method.getSignature().toString()))
                .toList();
        if (roots.isEmpty()) {
            throw new IllegalArgumentException(
                    "no concrete root methods matched " + rootClassName + "::" + methodSubstring);
        }

        DumpReachable exporter = new DumpReachable(view, rootType, maxMethods, maxDepth);
        roots.forEach(method -> exporter.enqueue(method.getSignature(), 0));
        exporter.enqueueClassInitializer(rootType, 0);
        exporter.run();

        List<Object> encoded = exporter.methods.values().stream()
                .sorted(Comparator.comparing(method -> method.getSignature().toString()))
                .map(method -> DumpJson.method(method, view))
                .toList();
        long classes = exporter.methods.values().stream()
                .map(method -> method.getSignature().getDeclClassType())
                .distinct()
                .count();
        return new Export(encoded, classes);
    }

    private void run() {
        while (!pending.isEmpty()) {
            Pending work = pending.removeFirst();
            Optional<? extends SootMethod> resolved = resolveConcrete(work.signature());
            if (resolved.isEmpty()) {
                continue;
            }
            SootMethod method = resolved.get();
            String key = method.getSignature().toString();
            if (methods.containsKey(key)) {
                continue;
            }
            if (methods.size() >= maxMethods) {
                throw new IllegalStateException(
                        "reachable-method limit exceeded (" + maxMethods + ") at " + key);
            }
            methods.put(key, method);
            if (work.depth() < maxDepth) {
                scan(method, work.depth());
            }
        }
    }

    private Optional<? extends SootMethod> resolveConcrete(MethodSignature signature) {
        Optional<? extends SootMethod> direct = view.getMethod(signature);
        if (direct.isPresent() && direct.get().isConcrete()) {
            return direct;
        }
        ClassType owner = signature.getDeclClassType();
        Set<String> visited = new HashSet<>();
        while (visited.add(owner.toString())) {
            Optional<? extends SootClass> klass = view.getClass(owner);
            if (klass.isEmpty()) {
                return Optional.empty();
            }
            Optional<? extends SootMethod> inherited = klass.get().getMethods().stream()
                    .filter(SootMethod::isConcrete)
                    .filter(method -> method.getName().equals(signature.getName()))
                    .filter(method -> method.getSignature().getType().equals(signature.getType()))
                    .filter(method -> method.getSignature().getParameterTypes().equals(signature.getParameterTypes()))
                    .findFirst();
            if (inherited.isPresent()) {
                return inherited;
            }
            Optional<? extends ClassType> superclass = klass.get().getSuperclass();
            if (superclass.isEmpty()) {
                return Optional.empty();
            }
            owner = superclass.get();
        }
        return Optional.empty();
    }

    private void scan(SootMethod method, int depth) {
        for (Stmt statement : method.getBody().getStmts()) {
            if (statement instanceof JAssignStmt assign) {
                scanValue(assign.getLeftOp(), depth, new HashSet<>());
                scanValue(assign.getRightOp(), depth, new HashSet<>());
            } else if (statement instanceof JIdentityStmt identity) {
                scanValue(identity.getLeftOp(), depth, new HashSet<>());
                scanValue(identity.getRightOp(), depth, new HashSet<>());
            } else if (statement instanceof JIfStmt branch) {
                scanValue(branch.getCondition(), depth, new HashSet<>());
            } else if (statement instanceof JInvokeStmt invoke) {
                invoke.getInvokeExpr().ifPresent(value -> scanValue(value, depth, new HashSet<>()));
            } else if (statement instanceof JReturnStmt returned) {
                scanValue(returned.getOp(), depth, new HashSet<>());
            } else if (statement instanceof JThrowStmt thrown) {
                scanValue(thrown.getOp(), depth, new HashSet<>());
            } else if (statement instanceof JSwitchStmt switched) {
                scanValue(switched.getKey(), depth, new HashSet<>());
            }
        }
    }

    private void scanValue(Value value, int depth, Set<Value> seen) {
        if (!seen.add(value)) {
            return;
        }
        int childDepth = depth + 1;
        if (value instanceof JStaticFieldRef field) {
            enqueueClassInitializer(field.getFieldSignature().getDeclClassType(), childDepth);
        }
        if (value instanceof JNewExpr allocation) {
            enqueueClassInitializer(allocation.getType(), childDepth);
        }
        if (value instanceof AbstractInvokeExpr invoke) {
            MethodSignature target = invoke.getMethodSignature();
            if (!(invoke instanceof JDynamicInvokeExpr)) {
                enqueue(target, childDepth);
                if (invoke instanceof JVirtualInvokeExpr || invoke instanceof JInterfaceInvokeExpr) {
                    enqueueRootRuntimeDispatchCandidate(target, childDepth);
                } else if (!(invoke instanceof AbstractInstanceInvokeExpr)) {
                    enqueueClassInitializer(target.getDeclClassType(), childDepth);
                }
            }
        }
        for (Value nested : value.getUses()) {
            scanValue(nested, depth, seen);
        }
    }

    /**
     * Walk only the root's known ancestors, not the entire input classpath. A missing
     * class stops that branch of the walk rather than inventing a subtype relation.
     */
    private Set<ClassType> ancestorsOf(ClassType type) {
        Set<ClassType> ancestors = new HashSet<>();
        ArrayDeque<ClassType> remaining = new ArrayDeque<>();
        remaining.add(type);
        while (!remaining.isEmpty()) {
            ClassType current = remaining.removeFirst();
            if (!ancestors.add(current)) {
                continue;
            }
            view.getClass(current).ifPresent(klass -> {
                klass.getSuperclass().ifPresent(remaining::addLast);
                remaining.addAll(klass.getInterfaces());
            });
        }
        return ancestors;
    }

    /**
     * Include a possible override for a virtual/interface call on the exact root
     * receiver. This is an export candidate, not proof that the actual call receiver
     * is the root object. The consumer must still resolve receiver provenance.
     */
    private void enqueueRootRuntimeDispatchCandidate(MethodSignature declared, int depth) {
        if (!rootAncestors.contains(declared.getDeclClassType())
                || !shouldAnalyzeOwner(declared.getDeclClassType().toString())) {
            return;
        }
        ClassType owner = rootRuntimeType;
        Set<ClassType> visited = new HashSet<>();
        while (visited.add(owner)) {
            Optional<? extends SootClass> klass = view.getClass(owner);
            if (klass.isEmpty()) {
                return;
            }
            Optional<? extends SootMethod> candidate = klass.get().getMethod(declared.getSubSignature());
            if (candidate.isPresent()) {
                SootMethod method = candidate.get();
                if (!method.isPrivate() && !method.isStatic()) {
                    if (method.isConcrete()) {
                        enqueue(method.getSignature(), depth);
                    }
                    // An abstract override must not fall through to a base implementation.
                    return;
                }
            }
            Optional<? extends ClassType> parent = klass.get().getSuperclass();
            if (parent.isEmpty()) {
                return;
            }
            owner = parent.get();
        }
    }

    private void enqueue(MethodSignature signature, int depth) {
        if (depth > maxDepth || !shouldAnalyzeOwner(signature.getDeclClassType().toString())) {
            return;
        }
        String key = signature.toString();
        if (queued.add(key)) {
            pending.addLast(new Pending(signature, depth));
        }
    }

    private void enqueueClassInitializer(ClassType owner, int depth) {
        String ownerName = owner.toString();
        if (depth > maxDepth
                || !shouldAnalyzeOwner(ownerName)
                || !initializerOwners.add(ownerName)) {
            return;
        }
        Optional<? extends SootClass> klass = view.getClass(owner);
        if (klass.isEmpty()) {
            return;
        }
        klass.get().getMethods().stream()
                .filter(SootMethod::isConcrete)
                .filter(method -> method.getName().equals("<clinit>"))
                .findFirst()
                .ifPresent(method -> enqueue(method.getSignature(), depth));
    }

    private static boolean shouldAnalyzeOwner(String owner) {
        return !(owner.startsWith("java.")
                || owner.startsWith("javax.")
                || owner.startsWith("jdk.")
                || owner.startsWith("sun.")
                || owner.startsWith("com.sun.")
                || owner.startsWith("com.google.common.")
                || owner.startsWith("it.unimi.dsi.fastutil.")
                || owner.startsWith("org.slf4j.")
                || owner.startsWith("org.apache.logging.")
                || owner.startsWith("net.minecraftforge.oredict.OreDictionary"));
    }
}
