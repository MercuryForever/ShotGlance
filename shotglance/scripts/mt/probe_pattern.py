"""看 FC1/FC2 的正确 int8 写法，作为改造 lm_head 的模板。"""

import os
import sys

import onnx

path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.expanduser("~"), "AppData", "Roaming", "ShotGlance",
    "models", "opus-mt-en-zh", "decoder.onnx"
)
m = onnx.load(path, load_external_data=False)


def walk(g, prefix=""):
    for n in g.node:
        yield prefix + (n.name or "-"), n
        for a in n.attribute:
            if a.type == onnx.AttributeProto.GRAPH:
                yield from walk(a.g, prefix + (n.name or "-") + "/")
            elif a.type == onnx.AttributeProto.GRAPHS:
                for i, sub in enumerate(a.graphs):
                    yield from walk(sub, prefix + (n.name or "-") + f"/g{i}/")


# 找第一个 DynamicQuantizeMatMul，把前后一步打出来
for name, n in walk(m.graph):
    if n.op_type != "DynamicQuantizeMatMul":
        continue
    print(f"=== 样例：{name} ===")
    print("  属性:", {a.name: onnx.helper.get_attribute_value(a) for a in n.attribute} or "(无)")
    for i, inp in enumerate(n.input):
        print(f"  in[{i}] {inp}")
    for o in n.output:
        print(f"  out {o}")
    print("  形状信息:")
    print(f"    A = {n.input[0]}  dims={[d.dim_value or d.dim_param for d in n.input[0] and []]}")
    break

# 打印 FC1 附近的完整链路
print("\n=== layers.0/fc1 链路 ===")
for name, n in walk(m.graph):
    if "layers.0/fc1" in name or "layers.0/self" in name:
        print(f"  [{n.op_type}] {name}")
        print(f"      in={list(n.input)}")
        print(f"      out={list(n.output)}")

# 输出形状（决定 A 是 2D 还是 3D）
print("\n=== 图输入输出 ===")
for i in m.graph.input:
    print(f"  in  {i.name}  {[d.dim_value or d.dim_param for d in i.type.tensor_type.shape.dim]}")
for o in m.graph.output:
    print(f"  out {o.name}  {[d.dim_value or d.dim_param for d in o.type.tensor_type.shape.dim]}")

# 找 If 的子图输入输出
print("\n=== If 节点 ===")
for name, n in walk(m.graph):
    if n.op_type != "If":
        continue
    print(f"  {name}  in={list(n.input)}  out={list(n.output)}")
    for a in n.attribute:
        if a.type == onnx.AttributeProto.GRAPH:
            print(f"    {a.name}: {len(a.g.node)} 个节点, "
                  f"{len(a.g.input)} 个输入, {len(a.g.output)} 个输出")
