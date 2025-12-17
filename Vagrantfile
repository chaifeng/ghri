# -*- mode: ruby -*-
# vi: set ft=ruby :

Vagrant.configure("2") do |config|
  config.vm.box = "bento/debian-13"

  config.vm.provider 'virtualbox' do |vb|
    vb.memory = '2048'
    vb.default_nic_type = "virtio"
  end

  config.vm.provider 'parallels' do |prl|
    prl.memory = '2048'
    prl.check_guest_tools = false
  end

  config.vm.provider 'vmware_desktop' do |vmware|
    vmware.memory = '2048'
  end

  config.vm.provision "Ensure cargo", privileged: false, type: "shell", inline: <<-SHELL
    if ! hash cargo &>/dev/null; then
      sudo apt update
      sudo apt install -y build-essential libssl-dev pkg-config
      sudo apt install -y coreutils git grep gzip make pre-commit sed tar unzip xz-utils
      curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | bash -s -- -y
      echo 'source "$HOME/.cargo/env"' >> ~/.bashrc
    fi
  SHELL

  config.vm.provision "Build ghri", privileged: false, type: "shell", inline: <<-SHELL
    cd /vagrant
    cargo test
    cargo build --release
  SHELL
end
