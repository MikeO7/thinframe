################################################################################
# aic8800: AIC8800DC SDIO Wi-Fi driver, the radxa-pkg source ormarch's wifi/build.sh pins,
# with the SDIO hunks of radxa's debian/patches series (same selection as wifi/build.sh).
################################################################################
AIC8800_VERSION = e9e43ed79c50483080e9bcdfb08caeeb869d7191
AIC8800_SITE = https://github.com/radxa-pkg/aic8800
AIC8800_SITE_METHOD = git
AIC8800_LICENSE = GPL-2.0
AIC8800_LICENSE_FILES = LICENSE
AIC8800_DRV = src/SDIO/driver_fw/driver
AIC8800_MODULE_SUBDIRS = $(AIC8800_DRV)/aic8800

# git apply in a private repository so paths are relative to the package directory.
define AIC8800_APPLY_SDIO_PATCHES
	cd $(@D) && git init -q . && \
	while read -r p; do \
		case "$$p" in ''|'#'*) continue ;; esac; \
		if grep -q "^+++ .*$(AIC8800_DRV)/" debian/patches/$$p; then \
			git apply --whitespace=nowarn --include='$(AIC8800_DRV)/*' debian/patches/$$p || exit 1; \
			echo "aic8800: applied $$p"; \
		fi; \
	done < debian/patches/series
endef
AIC8800_POST_PATCH_HOOKS += AIC8800_APPLY_SDIO_PATCHES

define AIC8800_INSTALL_FIRMWARE
	$(INSTALL) -d $(TARGET_DIR)/lib/firmware/aic8800_fw/SDIO/aic8800DC
	$(INSTALL) -m 0644 $(@D)/src/SDIO/driver_fw/fw/aic8800DC/* \
		$(TARGET_DIR)/lib/firmware/aic8800_fw/SDIO/aic8800DC/
endef
AIC8800_POST_INSTALL_TARGET_HOOKS += AIC8800_INSTALL_FIRMWARE

$(eval $(kernel-module))
$(eval $(generic-package))
