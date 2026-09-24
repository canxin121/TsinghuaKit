#
# To learn more about a Podspec see http://guides.cocoapods.org/syntax/podspec.html.
# Run `pod lib lint tsinghua_kit.podspec` to validate before publishing.
#
Pod::Spec.new do |s|
  s.name             = 'tsinghua_kit'
  s.version          = '0.1.0'
  s.summary          = 'Flutter FFI bridge for the TsinghuaKit Rust library'
  s.description      = <<-DESC
Builds the TsinghuaKit Rust library for Flutter applications.
                       DESC
  s.homepage         = 'https://github.com/canxin121/TsinghuaKit'
  s.license          = { :file => '../LICENSE' }
  s.author           = { 'TsinghuaKit contributors' => 'https://github.com/canxin121/TsinghuaKit' }

  # This will ensure the source files in Classes/ are included in the native
  # builds of apps using this FFI plugin. Podspec does not support relative
  # paths, so Classes contains a forwarder C file that relatively imports
  # `../src/*` so that the C sources can be shared among all target platforms.
  s.source           = { :path => '.' }
  s.source_files     = 'Classes/**/*'
  s.dependency 'FlutterMacOS'

  s.platform = :osx, '10.11'
  s.pod_target_xcconfig = { 'DEFINES_MODULE' => 'YES' }
  s.swift_version = '5.0'

  s.script_phase = {
    :name => 'Build Rust library',
    # First argument is relative path to the `rust` folder, second is name of rust library
    :script => 'sh "$PODS_TARGET_SRCROOT/../cargokit/build_pod.sh" ../rust tsinghua_kit',
    :execution_position => :before_compile,
    :input_files => ['${BUILT_PRODUCTS_DIR}/cargokit_phony'],
    # Let XCode know that the static library referenced in -force_load below is
    # created by this build step.
    :output_files => ["${BUILT_PRODUCTS_DIR}/libtsinghua_kit.a"],
  }
  s.pod_target_xcconfig = {
    'DEFINES_MODULE' => 'YES',
    # Flutter.framework does not contain a i386 slice.
    'EXCLUDED_ARCHS[sdk=iphonesimulator*]' => 'i386',
    'OTHER_LDFLAGS' => '-force_load ${BUILT_PRODUCTS_DIR}/libtsinghua_kit.a',
  }
end