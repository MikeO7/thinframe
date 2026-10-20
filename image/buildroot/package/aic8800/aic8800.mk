# STUB (TASK 2): AIC8800_VERSION/SITE must be the radxa commit ormarch's wifi/build.sh pins.
AIC8800_VERSION = TODO-pin
AIC8800_SITE = https://github.com/radxa-pkg/aic8800
AIC8800_SITE_METHOD = git
AIC8800_MODULE_SUBDIRS = drivers/aic8800
AIC8800_MODULE_MAKE_OPTS = CONFIG_AIC_WLAN_SUPPORT=m CONFIG_PLATFORM_ALLWINNER=y

$(eval $(kernel-module))
$(eval $(generic-package))
