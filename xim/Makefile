# lyyIme Mode B(lyyime-xim)构建脚本
#
# 目标:
#   all            = libxcb-imdkit.a + lyyime-xim + spike
#   libxcb-imdkit.a  vendor 源码直编(命令依据 xim/vendor/README.md,不用其 CMake
#                    以避开 ECM/UTHash 外部依赖;uthash 头文件搜索路径指向内置副本)
#   lyyime-xim     主程序(C99 + GTK3 + dlopen liblyyime_core.so)
#   spike          探路石最小 XIM server/客户端(xim/spike/)
#   test           单测:效果流 JSON 解析器(xim/tests/unit_effects_json)
#   e2e            Xvfb 全链路端到端(tests/e2e/xim_e2e.sh)
#   install/uninstall/clean
#
# 编译警告:-Wall -Wextra -Werror(0 警告红线)
# 借鉴出处:vendors 直编方式与 xcbimdkit_export.h 生成约定见 xim/vendor/README.md

# ---- 工具与参数 ----
CC      ?= gcc
CSTD    ?= c99
WARN    := -Wall -Wextra -Werror
CFLAGS  ?= -O2 -g
CFLAGS  += -std=$(CSTD) -D_GNU_SOURCE $(WARN)
# vendor 第三方源码:不适用 -Werror(红线约束的是自有交付代码;vendor 内文件
# 禁止修改,无法就地消警),仅保证可编译且产出与上游一致
VENDOR_CFLAGS := -O2 -g -std=gnu99 -D_GNU_SOURCE
AR      ?= ar

ROOT_DIR   := $(abspath $(dir $(lastword $(MAKEFILE_LIST))))
VENDOR_DIR := $(ROOT_DIR)/vendor/xcb-imdkit
VENDOR_SRC := $(VENDOR_DIR)/src
BUILD_DIR  := $(ROOT_DIR)/build
BIN_DIR    := $(BUILD_DIR)/bin
RES_DIR    := $(ROOT_DIR)/res
E2E_SH     := $(ROOT_DIR)/../tests/e2e/xim_e2e.sh

PKG_COMMON := $(shell pkg-config --cflags xcb xcb-util xcb-keysyms 2>/dev/null)
PKG_XIM    := $(shell pkg-config --libs xcb xcb-util xcb-keysyms 2>/dev/null)
PKG_GTK    := $(shell pkg-config --cflags --libs gtk+-3.0 2>/dev/null)

# vendor 源码清单 = 上游 CMakeLists(src/CMakeLists.txt L20-24):src/*.c +
# xlibi18n/lcCT.c lcUTF8.c lcCharSet.c(COMPOUND_TEXT 转码依赖后三者)
VENDOR_SRCS := $(notdir $(wildcard $(VENDOR_SRC)/*.c)) \
               lcCT.c lcUTF8.c lcCharSet.c
VENDOR_OBJS := $(addprefix $(BUILD_DIR)/vendor/,$(VENDOR_SRCS:.c=.o))
VENDOR_LIB  := $(VENDOR_DIR)/libxcb-imdkit.a

SRC_DIR := $(ROOT_DIR)/src
APP_SRCS := main.c common.c log.c config.c core_ffi.c effects_json.c \
            keysym_map.c xim_server.c candidate_window.c tray.c settings.c
APP_OBJS := $(addprefix $(BUILD_DIR)/app/,$(APP_SRCS:.c=.o))
APP_HDRS := $(wildcard $(SRC_DIR)/*.h)

.PHONY: all spike test e2e install uninstall clean help

all: $(VENDOR_LIB) $(BIN_DIR)/lyyime-xim spike

help:
	@echo "targets: all | libxcb-imdkit.a | lyyime-xim | spike | test | e2e | install | uninstall | clean"

# ---- vendor:xcb-imdkit 静态库 ----
# xcbimdkit_export.h 若缺失则按 CMake GenerateExportHeader 的默认形态补一个
$(VENDOR_DIR)/src/xcbimdkit_export.h:
	printf '#ifndef XCBIMDKIT_EXPORT_H\n#define XCBIMDKIT_EXPORT_H\n#define XCBIMDKIT_EXPORT\n#define XCBIMDKIT_NO_EXPORT\n#define XCBIMDKIT_DEPRECATED_EXPORT __attribute__((__deprecated__))\n#define XCBIMDKIT_DEPRECATED_NO_EXPORT\n#endif\n' > $@

$(BUILD_DIR)/vendor/%.o: $(VENDOR_SRC)/%.c | xcbimdkit_export.h
	@mkdir -p $(BUILD_DIR)/vendor
	$(CC) $(VENDOR_CFLAGS) -I$(VENDOR_SRC) -I$(VENDOR_DIR)/uthash \
		$(PKG_COMMON) -fvisibility=hidden -c $< -o $@

$(BUILD_DIR)/vendor/lcCT.o: $(VENDOR_SRC)/xlibi18n/lcCT.c | xcbimdkit_export.h
	@mkdir -p $(BUILD_DIR)/vendor
	$(CC) $(VENDOR_CFLAGS) -I$(VENDOR_SRC) -I$(VENDOR_SRC)/xlibi18n -I$(VENDOR_DIR)/uthash \
		$(PKG_COMMON) -fvisibility=hidden -c $< -o $@

$(BUILD_DIR)/vendor/lcUTF8.o: $(VENDOR_SRC)/xlibi18n/lcUTF8.c | xcbimdkit_export.h
	@mkdir -p $(BUILD_DIR)/vendor
	$(CC) $(VENDOR_CFLAGS) -I$(VENDOR_SRC) -I$(VENDOR_SRC)/xlibi18n -I$(VENDOR_DIR)/uthash \
		$(PKG_COMMON) -fvisibility=hidden -c $< -o $@

$(BUILD_DIR)/vendor/lcCharSet.o: $(VENDOR_SRC)/xlibi18n/lcCharSet.c | xcbimdkit_export.h
	@mkdir -p $(BUILD_DIR)/vendor
	$(CC) $(VENDOR_CFLAGS) -I$(VENDOR_SRC) -I$(VENDOR_SRC)/xlibi18n -I$(VENDOR_DIR)/uthash \
		$(PKG_COMMON) -fvisibility=hidden -c $< -o $@

$(VENDOR_LIB): $(VENDOR_OBJS)
	@rm -f $@
	$(AR) rcs $@ $^

xcbimdkit_export.h: $(VENDOR_DIR)/src/xcbimdkit_export.h
	@true

# ---- 主程序 lyyime-xim ----
# LYY_SRC_RES_DIR:开发态资源目录兜底(安装后走 /usr/local/share/lyyime/res)
$(BUILD_DIR)/app/%.o: $(SRC_DIR)/%.c $(APP_HDRS) | xcbimdkit_export.h
	@mkdir -p $(BUILD_DIR)/app
	$(CC) $(CFLAGS) -I$(SRC_DIR) -I$(VENDOR_SRC) $(PKG_COMMON) $(PKG_GTK) \
		-DLYY_SRC_RES_DIR='"$(ROOT_DIR)/res"' -c $< -o $@

$(BIN_DIR)/lyyime-xim: $(APP_OBJS) $(VENDOR_LIB)
	@mkdir -p $(BIN_DIR)
	$(CC) $(CFLAGS) -o $@ $(APP_OBJS) $(VENDOR_LIB) \
		$(PKG_XIM) $(PKG_GTK) -ldl -lX11

# ---- spike ----
$(BIN_DIR)/spike-server: $(ROOT_DIR)/spike/spike-server.c $(VENDOR_LIB)
	@mkdir -p $(BIN_DIR)
	$(CC) $(CFLAGS) -I$(VENDOR_SRC) -o $@ $< $(VENDOR_LIB) $(PKG_XIM)

$(BIN_DIR)/spike-client: $(ROOT_DIR)/spike/spike-client.c
	@mkdir -p $(BIN_DIR)
	$(CC) $(CFLAGS) -o $@ $< $(PKG_GTK)

spike: $(BIN_DIR)/spike-server $(BIN_DIR)/spike-client

spike-run: spike
	bash $(ROOT_DIR)/spike/run.sh

# ---- 单元测试(效果流 JSON 解析器,≥8 例)----
$(BUILD_DIR)/tests:
	@mkdir -p $(BUILD_DIR)/tests

$(BUILD_DIR)/tests/unit_effects_json: $(SRC_DIR)/effects_json.c $(ROOT_DIR)/tests/unit_effects_json.c | $(BUILD_DIR)/tests
	$(CC) $(CFLAGS) -I$(SRC_DIR) -o $@ $(SRC_DIR)/effects_json.c $(ROOT_DIR)/tests/unit_effects_json.c

$(BUILD_DIR)/tests/liblyyime_core_stub.so: $(ROOT_DIR)/tests/stub_lyyime_core.c
	@mkdir -p $(BUILD_DIR)/tests
	$(CC) $(CFLAGS) -shared -fPIC -o $@ $<

$(BUILD_DIR)/tests/e2e_client: $(ROOT_DIR)/tests/e2e_client.c
	@mkdir -p $(BUILD_DIR)/tests
	$(CC) $(CFLAGS) -o $@ $< $(PKG_GTK)

$(BUILD_DIR)/tests/unit_config: $(SRC_DIR)/config.c $(ROOT_DIR)/tests/unit_config.c | $(BUILD_DIR)/tests
	$(CC) $(CFLAGS) -I$(SRC_DIR) $(PKG_GTK) -o $@ $(SRC_DIR)/config.c $(ROOT_DIR)/tests/unit_config.c

test: $(BUILD_DIR)/tests/unit_effects_json $(BUILD_DIR)/tests/unit_config \
      $(BUILD_DIR)/tests/liblyyime_core_stub.so $(BUILD_DIR)/tests/e2e_client
	$(BUILD_DIR)/tests/unit_effects_json
	$(BUILD_DIR)/tests/unit_config
	@echo "单测全绿:effects_json + config + 桩库/e2e 客户端构建通过"

# ---- E2E(Xvfb :98 全自动断言)----
e2e: all test
	bash $(E2E_SH)

# ---- 安装(幂等)----
install: all
	bash $(ROOT_DIR)/install.sh

uninstall:
	bash $(ROOT_DIR)/uninstall.sh

clean:
	rm -rf $(BUILD_DIR)
	@# vendor 目录只清理我们直编出的静态库,不触碰其它文件
	rm -f $(VENDOR_LIB)
