"""定位 lm_head 附近的结构：shared.weight 是怎么被 dequantize 的。

用法：
    python probe_lmhead.py [decoder.onnx]
"""

import os
import sys

import onnx
from onnx import numpy_helper

path = sys.argv[1] if len(sys.argv) > 1 else os.path.join(
    os.path.expanduser("~"), "AppData", "Roaming", "ShotGlance",
    "models", "opus-mt-en-zh", "decoder.onnx"
)

m = onnx.load(path, load_external_data=False)
g = m.graph

inits = {i.name: i for i in g.initializer}
print(f"模型 {os.path.basename(path)}  {os.path.getsize(path) / 1048576:.1f} MB")
print(f"初始器 {len(inits)} 个，节点 {len(g.node)} 个\n")


def collect(graph, prefix=""):
    """含子图地收集所有节点。"""
    out = []
    for n in graph.node:
        out.append((prefix + n.name, n))
        for attr in n.attribute:
            if attr.type == onnx.AttributeProto.GRAPH:
                out.extend(collect(attr.g, prefix + n.name + "/then:"))
            elif attr.type == onnx.AttributeProto.GRAPHS:
                for i, sub in enumerate(attr.graphs):
                    out.extend(collect(sub, prefix + n.name + f"/gra[{i}]:"))
    return out


allnodes = collect(g)

print("=== 与 shared / lm_head 相关的节点 ===")
for name, n in allnodes:
    blob = (n.name + " " + " ".join(n.input) + " " + " ".join(n.output)).lower()
    if "shared" in blob or "lm_head" in blob:
        print(f"  [{n.op_type}] {name or '(无名)'}")
        for i in n.input:
            init = inits.get(i)
            if init is not None:
                tt = onnx.TensorProto.DataType.Name(init.data_type)
                print(f"      in  {i}  <init {tt} dims={list(init.dims)}>")
            else:
                print(f"      in  {i}")
        for o in n.output:
            print(f"      out {o}")

print("\n=== 所有 DequantizeLinear 的 scale / zero_point 来源 ===")
for name, n in allnodes:
    if n.op_type != "DequantizeLinear":
        continue
    src = n.input[0]
    init = inits.get(src)
    if init is None:
        continue
    dims = list(init.dims)
    # 只看大的（权重级），小的都是 per-tensor scale
    if not dims or max(dims) < 1000:
        continue
    tt = onnx.TensorProto.DataType.Name(init.data_type)
    scale = n.input[1] if len(n.input) > 1 else None
    zp = n.input[2] if len(n.input) > 2 else None
    sinit = inits.get(scale) if scale else None
    zinit = inits.get(zp) if zp else None
    print(f"  {name or '(无名)'}")
    print(f"    权重 {src} {tt} {dims}  = {init.ByteSize() / 1048576:.1f} MB")
    if sinit is not None:
        arr = numpy_helper.to_array(sinit)
        print(f"    scale {scale} shape={arr.shape} 范围=[{arr.min():.6g}, {arr.max():.6g}]")
    else:
        print(f"    scale {scale} (非初始器)")
    if zinit is not None:
        zarr = numpy_helper.to_array(zinit)
        print(f"    zero_point {zp} shape={zarr.shape} 全零={not zarr.any()}")
    else:
        print(f"    zero_point {zp}")

print("\n=== 谁消费 DequantizeLinear 的输出 ===")
producer = {}
for name, n in allnodes:
    for o in n.output:
        producer[o] = (name, n)
for name, n in allnodes:
    if n.op_type != "DequantizeLinear":
        continue
    src = n.input[0]
    init = inits.get(src)
    if init is None or not list(init.dims) or max(list(init.dims)) < 1000:
        continue
    out = n.output[0]
    consumers = [(nm, cn) for nm, cn in allnodes if out in cn.input]
    print(f"  {out} -> " + ", ".join(f"{cn.op_type}({nm or '无名'})" for nm, cn in consumers))
