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

# Do not offer WPA3 (SAE). The driver only does SAE with "external auth" from userspace, which
# iwd does not implement, so on a WPA2/WPA3 router every join timed out after 4 s (rwnx
# auth_type:3, measured on the frame 2026-10-22). Without the flag iwd picks WPA2-PSK.
define AIC8800_NO_SAE
	sed -i '/wiphy->features |= NL80211_FEATURE_SAE;/d' $(@D)/$(AIC8800_DRV)/aic8800/aic8800_fdrv/rwnx_main.c
	! grep -q 'NL80211_FEATURE_SAE;' $(@D)/$(AIC8800_DRV)/aic8800/aic8800_fdrv/rwnx_main.c
endef
AIC8800_POST_PATCH_HOOKS += AIC8800_NO_SAE

# A Wi-Fi address that is the same at every boot. With nothing in the chip's efuse the driver
# made one up at every load (88:00:33:77 and 16 random bits), so DHCP gave the frame a new IP
# each boot [V 2026-10-27]. mac-param.diff adds `mac=`; tfctl (netid.rs) derives it from the
# SoC's Security ID and loads the driver with it. An address in the efuse still wins.
# A .diff, not a .patch: Buildroot would apply a .patch before the debian series above.
define AIC8800_MAC_PARAM
	patch -d $(@D) -p1 --no-backup-if-mismatch < $(AIC8800_PKGDIR)/mac-param.diff
endef
AIC8800_POST_PATCH_HOOKS += AIC8800_MAC_PARAM

define AIC8800_INSTALL_FIRMWARE
	$(INSTALL) -d $(TARGET_DIR)/lib/firmware/aic8800_fw/SDIO/aic8800DC
	$(INSTALL) -m 0644 $(@D)/src/SDIO/driver_fw/fw/aic8800DC/* \
		$(TARGET_DIR)/lib/firmware/aic8800_fw/SDIO/aic8800DC/
endef
AIC8800_POST_INSTALL_TARGET_HOOKS += AIC8800_INSTALL_FIRMWARE

$(eval $(kernel-module))
$(eval $(generic-package))
