Name: linkrs
Version: 1.0.0
Release: 1%{?dist}
Summary: Linkrs graph database server
License: Apache-2.0
BuildArch: x86_64
Requires: systemd

%description
Lightweight single-node graph database service.

%files
%config(noreplace) /etc/linkrs/config.toml
/usr/local/bin/linkrs-server
/usr/local/bin/linkrs-cli
/etc/systemd/system/linkrs.service
