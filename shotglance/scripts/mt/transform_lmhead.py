"""把 lm_head 从「每步 Transpose + Dequantize 权重」改成「int8 直算」。

原结构（每个 decode 步都重跑一遍）：
    shared.weight (uint8 [65001,512], 33MB 常量)
      -> Transpose                33MB 转置
      -> DequantizeLinear         33MB -> 133MB fp32
      -> MatMul                   512x65001 fp32 GEMM
    实测每 token 约 8-11 ms，几乎全是浪费（权重根本不变）

改后：
    dql  = DynamicQuantizeLinear(A)                1x512，~0.01ms
    mm   = MatMulInteger(dql.q, W_T_q, azp, wzp)   int8 GEMM -> 1x65001 int32
    out  = Mul(Cast(mm), dql.scale * w.scale)      只反量化 1x65001
    out  = Add(out, bias)

代价：多一份转置后的 int8 权重（32MB）作为 initializer，模型 57MB -> 89MB。
换来：每步不再碰那 33MB，也不再有 133MB 的 fp32 临时张量。

实现要点（踩过的坑）：
  * If 的 then/else 各有**一套同名节点**，不能靠 id() 区分（protobuf
    repeated 容器每次访问都可能返回新 wrapper），一律按节点名匹配。
  * 新节点必须插在 MatMul 原来的位置，append 到末尾会破坏拓扑序
    （下游 Add 比它先出现）。

用法：
    python transform_lmhead.py --check           # 只打印会改什么
    python transform_lmhead.py --out out.onnx    # 写出新模型
"""

import argparse
import os
import sys

import numpy as np
import onnx
from onnx import helper, numpy_helper

DEFAULT_DIR = os.path.join(
    os.path.expanduser("~"), "AppData", "Roaming", "ShotGlance", "models", "opus-mt-en-zh"
)


def all_graphs(graph):
    """返回图及所有（递归）子图。"""
    out = [graph]
    for n in graph.node:
        for a in n.attribute:
            if a.type == onnx.AttributeProto.GRAPH:
                out.extend(all_graphs(a.g))
            elif a.type == onnx.AttributeProto.GRAPHS:
                for sub in a.graphs:
                    out.extend(all_graphs(sub))
    return out


def find_pattern(model):
    """找一个还没改造的 lm_head 三件套。

    返回 (graph, deq_name, tr_name, mm_name, weight_name) 或 None。
    """
    inits = {i.name: i for i in model.graph.initializer}
    for g in all_graphs(model.graph):
        by_name = {n.name: n for n in g.node}
        for n in g.node:
            if n.op_type != "DequantizeLinear" or not n.input:
                continue
            qname = n.input[0]
            tr = next((x for x in g.node if x.op_type == "Transpose" and qname in x.output), None)
            if tr is None:
                continue
            init = inits.get(tr.input[0])
            if init is None or init.data_type != onnx.TensorProto.UINT8:
                continue
            if len(init.dims) != 2 or init.dims[0] < 1000:
                continue
            dq = n.output[0]
            mm = next((x for x in g.node if x.op_type == "MatMul" and dq in x.input), None)
            if mm is None:
                continue
            assert tr.name in by_name and n.name in by_name and mm.name in by_name
            return g, n.name, tr.name, mm.name, tr.input[0]
    return None


def weight_info(model, node_name, graph):
    """取权重、scale、zero_point 名字。"""
    inits = {i.name: i for i in model.graph.initializer}
    node = next(n for n in graph.node if n.name == node_name)
    src = None
    for x in graph.node:
        if x.op_type == "Transpose" and x.name and node.input[0] in x.output:
            src = x.input[0]
            break
    scale = node.input[1]
    zp = node.input[2] if len(node.input) > 2 else None
    return src, scale, zp, inits


def apply_pattern(model, graph, deq_name, tr_name, mm_name, wt_name, counter):
    """把一处三件套换成 int8 直算，原地替换保拓扑序。"""
    inits = {i.name: i for i in model.graph.initializer}
    nodes = {n.name: n for n in graph.node}
    deq, tr, mm = nodes[deq_name], nodes[tr_name], nodes[mm_name]

    A = mm.input[0]
    out = mm.output[0]
    scale_name = deq.input[1]
    zp_name = deq.input[2] if len(deq.input) > 2 else None

    # 转置后的 int8 权重做成新 initializer（只加一次）
    wt_init_name = wt_name.replace("_quantized", "") + "_T_quantized"
    if wt_init_name not in inits:
        w = numpy_helper.to_array(inits[wt_name])
        # 校验转置的等价性：per-tensor 量化下转置与量化可交换
        assert w.ndim == 2
        wt = np.ascontiguousarray(w.T)
        model.graph.initializer.append(numpy_helper.from_array(wt, wt_init_name))
        s = float(numpy_helper.to_array(inits[scale_name]))
        z = float(numpy_helper.to_array(inits[zp_name])) if zp_name else 0.0
        print(f"  新增 {wt_init_name}  [{w.shape[0]}x{w.shape[1]}] -> "
              f"[{wt.shape[0]}x{wt.shape[1]}]  {wt.nbytes / 1048576:.1f} MB  "
              f"scale={s:.6g} zp={z:.1f}")

    counter[0] += 1
    k = counter[0]
    q = f"lmhead/a_quant_{k}"
    a_scale = f"lmhead/a_scale_{k}"
    a_zp = f"lmhead/a_zp_{k}"
    mm_out = f"lmhead/mm_{k}"
    cast_out = f"lmhead/cast_{k}"
    scl_mul = f"lmhead/scales_{k}"
    out_f = f"lmhead/deq_out_{k}"

    added = [
        helper.make_node("DynamicQuantizeLinear", [A], [q, a_scale, a_zp],
                         name=f"lmhead/dql_{k}"),
        helper.make_node("MatMulInteger", [q, wt_init_name, a_zp, zp_name], [mm_out],
                         name=f"lmhead/mmi_{k}"),
        helper.make_node("Cast", [mm_out], [cast_out], to=onnx.TensorProto.FLOAT,
                         name=f"lmhead/cast_node_{k}"),
        helper.make_node("Mul", [a_scale, scale_name], [scl_mul],
                         name=f"lmhead/scale_mul_{k}"),
        helper.make_node("Mul", [cast_out, scl_mul], [out_f], name=f"lmhead/deq_{k}"),
    ]

    dead = {deq_name, tr_name, mm_name}
    added_names = {n.name for n in added}

    # 下游改引用：原 MatMul 的输出 -> 新链的输出
    rebuilt = []
    for n in graph.node:
        if n.name in dead:
            if n.name == mm_name:
                rebuilt.extend(added)
            continue
        if n.name not in added_names:
            for i, inp in enumerate(n.input):
                if inp == out:
                    n.input[i] = out_f
        rebuilt.append(n)

    del graph.node[:]
    graph.node.extend(rebuilt)
    print(f"  替换 {mm_name}（A={A}）-> DynamicQuantizeLinear + MatMulInteger")


def transform(model):
    # 先确认权重形状，把代价说清楚
    first = find_pattern(model)
    if first is None:
        sys.exit("没找到 lm_head 的坏模式，模型可能已经改过了")

    counter = [0]
    while True:
        found = find_pattern(model)
        if found is None:
            break
        apply_pattern(model, *found, counter)
        if counter[0] > 8:
            sys.exit("替换次数异常，中止")
    return counter[0]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--src", default=os.path.join(DEFAULT_DIR, "decoder.onnx"))
    ap.add_argument("--out", default=os.path.join(DEFAULT_DIR, "decoder.onnx"))
    ap.add_argument("--check", action="store_true")
    args = ap.parse_args()

    print(f"源模型：{args.src}  {os.path.getsize(args.src) / 1048576:.1f} MB")
    model = onnx.load(args.src, load_external_data=False)

    first = find_pattern(model)
    if first is None:
        sys.exit("没找到 lm_head 的坏模式")
    print(f"首次定位：graph 内 {first[3]} 消费 {first[2]} -> {first[1]}")

    if args.check:
        g, deq_name, tr_name, mm_name, wt = first
        ini = {i.name: i for i in model.graph.initializer}
        w = numpy_helper.to_array(ini[wt])
        deq = next(n for n in g.node if n.name == deq_name)
        s = float(numpy_helper.to_array(ini[deq.input[1]]))
        z = float(numpy_helper.to_array(ini[deq.input[2]]))
        print(f"\n权重 {wt}  {list(w.shape)} uint8  {w.nbytes / 1048576:.1f} MB")
        print(f"  scale={s:.6g}  zero_point={z}  （per-tensor，转置与量化可交换）")
        print(f"  转置后 initializer 占 {w.nbytes / 1048576:.1f} MB")
        print(f"  模型体积 {os.path.getsize(args.src) / 1048576:.1f} -> "
              f"约 {os.path.getsize(args.src) / 1048576 + w.nbytes / 1048576:.1f} MB")
        print("\n（--check 模式，未写文件）")
        return

    n = transform(model)
    print(f"\n共替换 {n} 处")
    onnx.checker.check_model(model)
    print("ONNX checker 通过")
    tmp = args.out + ".tmp"
    onnx.save(model, tmp)
    os.replace(tmp, args.out)
    print(f"已写出：{args.out}  {os.path.getsize(args.out) / 1048576:.1f} MB")


if __name__ == "__main__":
    main()
