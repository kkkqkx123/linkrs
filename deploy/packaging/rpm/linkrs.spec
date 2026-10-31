Name:           linkrs
Version: 0.1.0
Release:        1%{?dist}
Summary:        Linkrs graph database server
License:        Apache-2.0
BuildArch:      x86_64
Requires:       systemd
BuildRequires:  systemd-rpm-macros

%description
Lightweight single-node graph database service.

%pre
getent group linkrs >/dev/null || groupadd --system linkrs
getent passwd linkrs >/dev/null || useradd --system --gid linkrs \
    --home-dir /var/lib/linkrs --create-home --shell /usr/sbin/nologin linkrs
install -d -o linkrs -g linkrs -m 0750 /var/lib/linkrs /var/log/linkrs /etc/linkrs
exit 0

%post
%systemd_post linkrs.service

%preun
%systemd_preun linkrs.service

%postun
%systemd_postun linkrs.service

%files
%dir %attr(0750,linkrs,linkrs) /etc/linkrs
%config(noreplace) %attr(0640,linkrs,linkrs) /etc/linkrs/config.toml
%dir %attr(0750,linkrs,linkrs) /var/lib/linkrs
%dir %attr(0750,linkrs,linkrs) /var/log/linkrs
%{_bindir}/linkrs-server
%{_bindir}/linkrs-cli
%{_unitdir}/linkrs.service

%changelog
* Sat Oct 31 2026 Linkrs Contributors <linkrs@example.com> - 0.1.0-1
- Align scriptlets with the DEB behavior (preset on install, stop on upgrade,
  daemon-reload via systemd macros).
- Use RPM macro paths (%{_bindir}, %{_unitdir}) instead of /usr/local.
- Ship the deploy/config/linkrs.toml template as /etc/linkrs/config.toml.
