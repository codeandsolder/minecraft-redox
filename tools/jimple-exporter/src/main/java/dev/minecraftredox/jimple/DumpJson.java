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
import sootup.core.jimple.common.Local;
import sootup.core.jimple.common.Value;
import sootup.core.jimple.common.constant.ClassConstant;
import sootup.core.jimple.common.constant.DoubleConstant;
import sootup.core.jimple.common.constant.FloatConstant;
import sootup.core.jimple.common.constant.IntConstant;
import sootup.core.jimple.common.constant.LongConstant;
import sootup.core.jimple.common.constant.MethodHandle;
import sootup.core.jimple.common.constant.MethodType;
import sootup.core.jimple.common.constant.NullConstant;
import sootup.core.jimple.common.constant.StringConstant;
import sootup.core.jimple.common.expr.AbstractBinopExpr;
import sootup.core.jimple.common.expr.AbstractInstanceInvokeExpr;
import sootup.core.jimple.common.expr.AbstractInvokeExpr;
import sootup.core.jimple.common.expr.AbstractUnopExpr;
import sootup.core.jimple.common.expr.JCastExpr;
import sootup.core.jimple.common.expr.JDynamicInvokeExpr;
import sootup.core.jimple.common.expr.JInstanceOfExpr;
import sootup.core.jimple.common.expr.JLengthExpr;
import sootup.core.jimple.common.expr.JNegExpr;
import sootup.core.jimple.common.expr.JNewArrayExpr;
import sootup.core.jimple.common.expr.JNewExpr;
import sootup.core.jimple.common.expr.JNewMultiArrayExpr;
import sootup.core.jimple.common.expr.JPhiExpr;
import sootup.core.jimple.common.ref.JArrayRef;
import sootup.core.jimple.common.ref.JInstanceFieldRef;
import sootup.core.jimple.common.ref.JParameterRef;
import sootup.core.jimple.common.ref.JStaticFieldRef;
import sootup.core.jimple.common.ref.JThisRef;
import sootup.core.jimple.common.stmt.JAssignStmt;
import sootup.core.jimple.common.stmt.JGotoStmt;
import sootup.core.jimple.common.stmt.JIdentityStmt;
import sootup.core.jimple.common.stmt.JIfStmt;
import sootup.core.jimple.common.stmt.JInvokeStmt;
import sootup.core.jimple.common.stmt.JReturnStmt;
import sootup.core.jimple.common.stmt.JReturnVoidStmt;
import sootup.core.jimple.common.stmt.JThrowStmt;
import sootup.core.jimple.common.stmt.Stmt;
import sootup.core.jimple.javabytecode.stmt.JSwitchStmt;
import sootup.core.model.Body;
import sootup.core.model.SootClass;
import sootup.core.model.SootMethod;
import sootup.core.signatures.FieldSignature;
import sootup.core.signatures.MethodSignature;
import sootup.core.types.ClassType;
import sootup.core.views.View;
import sootup.java.bytecode.frontend.inputlocation.JavaClassPathAnalysisInputLocation;
import sootup.java.core.views.JavaView;

public final class DumpJson {
    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    private DumpJson() {}

    public static void main(String[] args) {
        if (args.length < 2 || args.length > 3) {
            throw new IllegalArgumentException("usage: <class-path> <class-name> [method-name-substring]");
        }
        AnalysisInputLocation input = new JavaClassPathAnalysisInputLocation(args[0]);
        View view = new JavaView(input);
        ClassType type = view.getIdentifierFactory().getClassType(args[1]);
        SootClass klass = view.getClass(type).orElseThrow();
        String filter = args.length == 3 ? args[2] : "";
        List<Object> methods = klass.getMethods().stream()
                .filter(method -> method.isConcrete() && method.getName().contains(filter))
                .sorted(Comparator.comparing(method -> method.getSignature().toString()))
                .map(method -> DumpJson.method(method, view))
                .toList();
        System.out.println(GSON.toJson(methods));
    }

    static Object method(SootMethod method, View view) {
        Body body = method.getBody();
        List<Stmt> statements = body.getStmts();
        Map<Stmt, Integer> ids = new IdentityHashMap<>();
        for (int index = 0; index < statements.size(); index++) {
            ids.put(statements.get(index), index);
        }
        ControlFlowGraph<?> cfg = body.getControlFlowGraph();
        List<Object> encoded = new ArrayList<>();
        for (int index = 0; index < statements.size(); index++) {
            Stmt statement = statements.get(index);
            Map<String, Object> row = new LinkedHashMap<>();
            row.put("id", index);
            row.put("kind", statement.getClass().getSimpleName());
            row.put("text", statement.toString());
            row.put("def", statement.getDef().map(Object::toString).orElse(null));
            row.put("uses", statement.getUses().stream().map(DumpJson::valueSummary).toList());
            row.put("successors", cfg.successors(statement).stream().map(ids::get).toList());
            row.put("operation", statement(statement, body, ids));
            encoded.add(row);
        }
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("signature", methodSignature(method.getSignature()));
        result.put("display_signature", method.getSignature().toString());
        view.getClass(method.getSignature().getDeclClassType()).ifPresent(klass -> {
            result.put(
                    "declaring_superclass",
                    klass.getSuperclass().map(Object::toString).orElse(null));
            result.put(
                    "declaring_interfaces",
                    klass.getInterfaces().stream().map(Object::toString).sorted().toList());
        });
        result.put("statements", encoded);
        return result;
    }

    private static Object statement(Stmt statement, Body body, Map<Stmt, Integer> ids) {
        Map<String, Object> result = new LinkedHashMap<>();
        if (statement instanceof JAssignStmt assign) {
            result.put("kind", "assign");
            result.put("left", value(assign.getLeftOp()));
            result.put("right", value(assign.getRightOp()));
        } else if (statement instanceof JIdentityStmt identity) {
            result.put("kind", "identity");
            result.put("left", value(identity.getLeftOp()));
            result.put("right", value(identity.getRightOp()));
        } else if (statement instanceof JIfStmt branch) {
            result.put("kind", "if");
            result.put("condition", value(branch.getCondition()));
        } else if (statement instanceof JInvokeStmt invoke) {
            result.put("kind", "invoke");
            result.put("expression", invoke.getInvokeExpr().map(DumpJson::value).orElse(null));
        } else if (statement instanceof JReturnStmt returned) {
            result.put("kind", "return");
            result.put("value", value(returned.getOp()));
        } else if (statement instanceof JReturnVoidStmt) {
            result.put("kind", "return_void");
        } else if (statement instanceof JThrowStmt thrown) {
            result.put("kind", "throw");
            result.put("value", value(thrown.getOp()));
        } else if (statement instanceof JSwitchStmt switched) {
            result.put("kind", "switch");
            result.put("key", value(switched.getKey()));
            List<Stmt> targets = switched.getTargetStmts(body);
            List<Object> cases = new ArrayList<>();
            List<IntConstant> values = switched.getValues();
            for (int index = 0; index < values.size(); index++) {
                Map<String, Object> branch = new LinkedHashMap<>();
                branch.put("value", values.get(index).getValue());
                branch.put("target", ids.get(targets.get(index)));
                cases.add(branch);
            }
            result.put("cases", cases);
            result.put("default_target", switched.getDefaultTarget(body).map(ids::get).orElse(null));
        } else if (statement instanceof JGotoStmt) {
            result.put("kind", "goto");
        } else {
            result.put("kind", "opaque");
        }
        return result;
    }

    private static Object valueSummary(Value value) {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("kind", value.getClass().getSimpleName());
        result.put("type", value.getType().toString());
        result.put("text", value.toString());
        return result;
    }

    private static Object value(Value value) {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("jimple_kind", value.getClass().getSimpleName());
        result.put("type", value.getType().toString());
        result.put("text", value.toString());

        if (value instanceof Local local) {
            result.put("kind", "local");
            result.put("name", local.getName());
        } else if (value instanceof IntConstant constant) {
            result.put("kind", "integer_constant");
            result.put("value", constant.getValue());
        } else if (value instanceof LongConstant constant) {
            result.put("kind", "integer_constant");
            result.put("value", constant.getValue());
        } else if (value instanceof FloatConstant constant) {
            result.put("kind", "float_constant");
            result.put("value", constant.getValue());
        } else if (value instanceof DoubleConstant constant) {
            result.put("kind", "float_constant");
            result.put("value", constant.getValue());
        } else if (value instanceof StringConstant constant) {
            result.put("kind", "string_constant");
            result.put("value", constant.getValue());
        } else if (value instanceof ClassConstant constant) {
            result.put("kind", "class_constant");
            result.put("class_type", decodeClassConstant(constant.getValue()));
        } else if (value instanceof MethodHandle handle
                && handle.isMethodRef()
                && handle.getReferenceSignature() instanceof MethodSignature signature) {
            result.put("kind", "method_handle");
            result.put("handle_kind", handle.getKind().toString());
            result.put("method", methodSignature(signature));
        } else if (value instanceof MethodType methodType) {
            result.put("kind", "method_type");
            result.put("return_type", methodType.getReturnType().toString());
            result.put(
                    "parameter_types",
                    methodType.getParameterTypes().stream().map(Object::toString).toList());
        } else if (value instanceof NullConstant) {
            result.put("kind", "null_constant");
        } else if (value instanceof JParameterRef parameter) {
            result.put("kind", "parameter");
            result.put("index", parameter.getIndex());
        } else if (value instanceof JThisRef) {
            result.put("kind", "this");
        } else if (value instanceof JArrayRef array) {
            result.put("kind", "array_ref");
            result.put("base", value(array.getBase()));
            result.put("index", value(array.getIndex()));
        } else if (value instanceof JInstanceFieldRef field) {
            result.put("kind", "instance_field");
            result.put("base", value(field.getBase()));
            result.put("field", fieldSignature(field.getFieldSignature()));
        } else if (value instanceof JStaticFieldRef field) {
            result.put("kind", "static_field");
            result.put("field", fieldSignature(field.getFieldSignature()));
        } else if (value instanceof AbstractBinopExpr binary) {
            result.put("kind", "binary");
            result.put("operator", binary.getSymbol());
            result.put("left", value(binary.getOp1()));
            result.put("right", value(binary.getOp2()));
        } else if (value instanceof JCastExpr cast) {
            result.put("kind", "cast");
            result.put("cast_type", cast.getType().toString());
            result.put("value", value(cast.getOp()));
        } else if (value instanceof JInstanceOfExpr instanceOf) {
            result.put("kind", "instance_of");
            result.put("value", value(instanceOf.getOp()));
            result.put("check_type", instanceOf.getCheckType().toString());
        } else if (value instanceof JNegExpr negated) {
            result.put("kind", "unary");
            result.put("operator", "-");
            result.put("value", value(negated.getOp()));
        } else if (value instanceof JLengthExpr length) {
            result.put("kind", "array_length");
            result.put("value", value(length.getOp()));
        } else if (value instanceof JNewExpr allocation) {
            result.put("kind", "new");
            result.put("class_type", allocation.getType().toString());
        } else if (value instanceof JNewArrayExpr allocation) {
            result.put("kind", "new_array");
            result.put("element_type", allocation.getBaseType().toString());
            result.put("size", value(allocation.getSize()));
        } else if (value instanceof JNewMultiArrayExpr allocation) {
            result.put("kind", "new_multi_array");
            result.put("array_type", allocation.getBaseType().toString());
            result.put("sizes", allocation.getSizes().stream().map(DumpJson::value).toList());
        } else if (value instanceof JPhiExpr phi) {
            result.put("kind", "phi");
            result.put("values", phi.getArgs().stream().map(DumpJson::value).toList());
        } else if (value instanceof AbstractInvokeExpr invoke) {
            result.put("kind", "invoke");
            result.put("invoke_kind", value.getClass().getSimpleName());
            result.put("method", methodSignature(invoke.getMethodSignature()));
            if (invoke instanceof AbstractInstanceInvokeExpr instance) {
                result.put("base", value(instance.getBase()));
            }
            result.put("arguments", invoke.getArgs().stream().map(DumpJson::value).toList());
            if (invoke instanceof JDynamicInvokeExpr dynamic) {
                result.put("bootstrap_method", methodSignature(dynamic.getBootstrapMethodSignature()));
                result.put("bootstrap_arguments", dynamic.getBootstrapArgs().stream().map(DumpJson::value).toList());
            }
        } else if (value instanceof AbstractUnopExpr unary) {
            result.put("kind", "unary");
            result.put("operator", value.getClass().getSimpleName());
            result.put("value", value(unary.getOp()));
        } else {
            result.put("kind", "opaque");
        }
        return result;
    }

    private static Object fieldSignature(FieldSignature signature) {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("owner", signature.getDeclClassType().toString());
        result.put("name", signature.getName());
        result.put("type", signature.getType().toString());
        return result;
    }

    private static String decodeClassConstant(String descriptor) {
        if (descriptor.startsWith("[")) {
            return switch (descriptor.charAt(1)) {
                case 'L' -> decodeClassConstant(descriptor.substring(1)) + "[]";
                case 'Z' -> "boolean[]";
                case 'B' -> "byte[]";
                case 'C' -> "char[]";
                case 'S' -> "short[]";
                case 'I' -> "int[]";
                case 'J' -> "long[]";
                case 'F' -> "float[]";
                case 'D' -> "double[]";
                default -> descriptor;
            };
        }
        if (descriptor.startsWith("L") && descriptor.endsWith(";")) {
            return descriptor.substring(1, descriptor.length() - 1).replace('/', '.');
        }
        return descriptor.replace('/', '.');
    }

    private static Object methodSignature(MethodSignature signature) {
        Map<String, Object> result = new LinkedHashMap<>();
        result.put("owner", signature.getDeclClassType().toString());
        result.put("name", signature.getName());
        result.put("return_type", signature.getType().toString());
        result.put("parameter_types", signature.getParameterTypes().stream().map(Object::toString).toList());
        return result;
    }
}
