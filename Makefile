# Development workflow for the openDAQ Rust bindings, mirroring cl-opendaq.
#
# `make bindings` clones the pinned openDAQ source, builds the native
# libraries (requires CMake + a C++ toolchain), copies them into
# bin/$(OPENDAQ_RUNTIME_TRIPLE)/, and regenerates the generated Rust sources.
# End users never need this: released crates download prebuilt binaries.

OPENDAQ_REPO_URL ?= https://github.com/adolenc/openDAQ.git
OPENDAQ_REF ?= c-bindings-docstrings

OPENDAQ_SOURCE_DIR ?= tmp/openDAQ
OPENDAQ_BUILD_DIR ?= tmp/build
OPENDAQ_RUNTIME_TRIPLE ?= linux-x64

OPENDAQ_CMAKE_ARGS ?= \
	-DOPENDAQ_GENERATE_C_BINDINGS=ON \
	-DOPENDAQ_GENERATE_PYTHON_BINDINGS=OFF \
	-DOPENDAQ_GENERATE_DELPHI_BINDINGS=OFF \
	-DOPENDAQ_GENERATE_CSHARP_BINDINGS=OFF \
	-DDAQMODULES_REF_DEVICE_MODULE=ON \
	-DDAQMODULES_REF_FB_MODULE=ON \
	-DDAQMODULES_REF_FB_MODULE_ENABLE_RENDERER=OFF \
	-DOPENDAQ_ENABLE_TESTS=OFF \
	-DOPENDAQ_ENABLE_TEST_UTILS=OFF \
	-DOPENDAQ_ENABLE_ACCESS_CONTROL=OFF \
	-DOPENDAQ_ENABLE_NATIVE_STREAMING=ON \
	-DDAQMODULES_OPENDAQ_CLIENT_MODULE=ON \
	-DBOOST_LOCALE_ENABLE_ICU=OFF

.PHONY: bindings clone-opendaq build-native regenerate-bindings test examples clean

bindings: clone-opendaq build-native regenerate-bindings

clone-opendaq:
	rm -rf $(OPENDAQ_SOURCE_DIR)
	mkdir -p $(dir $(OPENDAQ_SOURCE_DIR))
	git clone $(OPENDAQ_REPO_URL) $(OPENDAQ_SOURCE_DIR)
	git -C $(OPENDAQ_SOURCE_DIR) checkout --force $(OPENDAQ_REF)

build-native:
	cmake -S $(OPENDAQ_SOURCE_DIR) -B $(OPENDAQ_BUILD_DIR) \
		-DCMAKE_BUILD_TYPE=Release $(OPENDAQ_CMAKE_ARGS)
	cmake --build $(OPENDAQ_BUILD_DIR) --config Release
	mkdir -p bin/$(OPENDAQ_RUNTIME_TRIPLE)
	find $(OPENDAQ_BUILD_DIR) -type f -path '*/bin/*' \
		\( -name '*.so' -o -name '*.dylib' -o -name '*.dll' \) \
		! -name 'opendaq.cpython*' ! -name '*_test_dll.so' \
		-exec cp {} bin/$(OPENDAQ_RUNTIME_TRIPLE)/ \;

regenerate-bindings:
	python3 tools/generate_bindings.py --opendaq-repo $(OPENDAQ_SOURCE_DIR)

test:
	cargo test

examples:
	cargo build --examples

clean:
	rm -rf tmp dist
	cargo clean
