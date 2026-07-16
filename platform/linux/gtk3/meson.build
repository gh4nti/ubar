project('ubar-gtk3-shell', 'c', default_options: ['c_std=c17', 'warning_level=3'])

gtk = dependency('gtk+-3.0', version: '>=3.24')
dl = meson.get_compiler('c').find_library('dl', required: false)

executable(
  'ubar-gtk3-shell',
  'src/main.c',
  dependencies: [gtk, dl],
  include_directories: include_directories('../../../include'),
  install: true,
)
