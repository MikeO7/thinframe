################################################################################
# tfctl: the prebuilt static armv7 musl binary. image/build.sh rootfs builds it with
# `cargo zigbuild` on the Mac and stages it in image/work/tfctl/ before Buildroot runs.
################################################################################
TFCTL_VERSION = local
TFCTL_SITE = $(BR2_EXTERNAL_THINFRAME_PATH)/../work/tfctl
TFCTL_SITE_METHOD = local
TFCTL_LICENSE = MIT

define TFCTL_INSTALL_TARGET_CMDS
	$(INSTALL) -D -m 0755 $(@D)/tfctl $(TARGET_DIR)/usr/bin/tfctl
endef

$(eval $(generic-package))
