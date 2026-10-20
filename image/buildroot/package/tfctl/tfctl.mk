# Installs the prebuilt binary: cargo zigbuild --release --target armv7-unknown-linux-musleabihf
TFCTL_VERSION = local
TFCTL_SITE = $(BR2_EXTERNAL_THINFRAME_PATH)/../../tfctl
TFCTL_SITE_METHOD = local
TFCTL_BIN = target/armv7-unknown-linux-musleabihf/release/tfctl

define TFCTL_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/$(TFCTL_BIN) $(TARGET_DIR)/usr/bin/tfctl
endef

$(eval $(generic-package))
