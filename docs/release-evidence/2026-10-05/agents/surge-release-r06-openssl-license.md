# OpenSSL static redistribution license evidence

Local Homebrew installed OpenSSL is /opt/homebrew/Cellar/openssl@3/3.6.4. LICENSE.txt contains complete Apache2 terms; repository LICENSE-APACHE contains only a 17-line short usage notice, not full terms.

Primary upstream: https://raw.githubusercontent.com/openssl/openssl/openssl-3.6.4/LICENSE.txt . Apache2 section4(a) calls for giving recipients a copy of license; section4(d) imposes NOTICE inclusion only where upstream includes NOTICE. GitHub official recursive tree openssl-3.6.4 confirmed root LICENSE.txt and no NOTICE anywhere (external Perl build helper LICENSE unrelated to linked libraries). No OpenSSL NOTICE file requirement was found for this release; no old OpenSSL/SSLeay acknowledgement condition applies to current Apache2 license.

Minimum proposal: replace existing LICENSE-APACHE short notice with complete Apache2 terms, retaining original Surge copyright/usage statement; README License paragraph attributes linked OpenSSL3 and upstream source. Existing archive's LICENSE-APACHE+README members carry both, preserving exactly five-member package contract. Packaging tests and actual archive license byte inspection required after change. No claim general dependency notice audit closure: task scoped OpenSSL.

Coordinator accepted minimal license closure. Expanded LICENSE-APACHE to complete upstream terms plus original Surge usage/copyright notice; README attributes static OpenSSL3 and links source/license. Official GitHub LICENSE bytes downloaded via contents API match installed Homebrew LICENSE exactly and prefix of final LICENSE-APACHE; Surge copyright retained. No archive member changes/no Cargo changes.
