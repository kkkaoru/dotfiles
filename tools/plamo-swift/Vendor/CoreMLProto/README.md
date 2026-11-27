# Core ML protobuf schema, vendored as data

These `.proto` files are copied unmodified from `apple/coremltools`, `mlmodel/format/`
(`git sparse-checkout set mlmodel/format`). They are schema data, not code: nothing here is
executed, and no Python package is installed or imported at build or run time.

They exist so an ML Program can be **authored without Python**. `protoc` (a C++ binary) encodes a
text-format `CoreML.Specification.Model` into `model.mlmodel`; `coremlc` compiles that into
`model.mlmodelc`; Swift loads it with `MLModel`. The generator that writes the text format is Swift.

```sh
protoc --proto_path=Vendor/CoreMLProto --encode=CoreML.Specification.Model \
  Vendor/CoreMLProto/Model.proto < program.prototxt > model.mlmodel
xcrun coremlc compile model.mlpackage out/
```

An `.mlpackage` is `Manifest.json` plus `Data/com.apple.CoreML/model.mlmodel`. Two requirements cost
a debugging round each: `specificationVersion` must be at least 7 for `FLOAT16` multiarray features,
and `matmul` takes `transpose_x`/`transpose_y` as **inputs** holding a `BOOL` immediate, not as
attributes.

Verified end to end: one `matmul` (1×1536 by 1536×1536, fp16) compiled and ran on `CPU_AND_NE` in
0.871 ms, i.e. 5.4 GB/s against that weight matrix.
