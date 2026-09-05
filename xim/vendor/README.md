# xim/vendor — Mode B 第三方源码

## xcb-imdkit(fcitx)

- 来源:https://github.com/fcitx/xcb-imdkit,master @ `44f5c8219bca`(2026-09-06 vendor,已去除 .git)
- 许可:**LGPL-2.1-only**(见 `xcb-imdkit/LICENSES/`)——本项目以动态链接方式使用,不修改其源码;如有修改需在 `xim/vendor/patches/` 以补丁形式维护并在本文件登记
- 用途:`lyyime-xim` 的 XIM 协议 server 端实现(fcitx5 XIM 前端同款)
- 构建方式:不用其 CMakeLists(避免 ECM/UTHash 外部依赖),由 `xim/Makefile` 直接编译 **`src/{parser,ximproto,imdkit,protocolhandler,message,common,imclient,clientprotocolhandler,encoding}.c + src/xlibi18n/{lcCT,lcUTF8,lcCharSet}.c + lcUniConv/*.c(排除 *_tab_to_h.c 生成器)`**(与上游 CMakeLists 清单一致;lcCT/lcUTF8/lcCharSet 是 COMPOUND_TEXT 转码依赖,不可省);需先写 `src/xcbimdkit_export.h`(静态构建版 export 宏,Makefile 有生成兜底);头文件搜索路径指向 `xcb-imdkit/uthash/`(uthash.h 直在根下,无 src 子目录);系统依赖:`libxcb-devel`、`xcb-util-devel`(xcb_atom.h)、`xcb-util-keysyms-devel`(xcb_keysyms.h),pkg-config 包 `xcb xcb-atom xcb-keysyms`
- 前例:uim-xim / scim / fcitx 系均内置同源 IMdkit 家族代码;现代维护版即本仓库
