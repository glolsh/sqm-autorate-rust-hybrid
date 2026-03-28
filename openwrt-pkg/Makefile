include $(TOPDIR)/rules.mk

PKG_NAME:=sqm-autorate
PKG_VERSION:=0.2.0
PKG_RELEASE:=1

PKG_MAINTAINER:=Your Name <your.email@example.com>
PKG_LICENSE:=MIT

include $(INCLUDE_DIR)/package.mk

define Package/sqm-autorate
  SECTION:=net
  CATEGORY:=Network
  TITLE:=SQM Autorate (Rust)
  DEPENDS:=+tc +kmod-sched-cake
  PKGARCH:=aarch64_generic
endef

define Package/sqm-autorate/description
  A Rust implementation of sqm-autorate to dynamically adjust SQM settings.
endef

define Package/sqm-autorate/conffiles
/etc/config/sqm-autorate
endef

define Build/Prepare
	mkdir -p $(PKG_BUILD_DIR)
	cp ./sqm-autorate-rust $(PKG_BUILD_DIR)/
	cp ./sqm-autorate.init $(PKG_BUILD_DIR)/
	cp ./sqm-autorate.config $(PKG_BUILD_DIR)/
endef

define Build/Compile
	# Doing nothing, pre-compiled binary is used
endef

define Package/sqm-autorate/install
	$(INSTALL_DIR) $(1)/usr/bin
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/sqm-autorate-rust $(1)/usr/bin/sqm-autorate-rust

	$(INSTALL_DIR) $(1)/etc/init.d
	$(INSTALL_BIN) $(PKG_BUILD_DIR)/sqm-autorate.init $(1)/etc/init.d/sqm-autorate

	$(INSTALL_DIR) $(1)/etc/config
	$(INSTALL_CONF) $(PKG_BUILD_DIR)/sqm-autorate.config $(1)/etc/config/sqm-autorate
endef

$(eval $(call BuildPackage,sqm-autorate))
